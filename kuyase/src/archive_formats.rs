use crate::ext_fs::{read_ext2_image, EXT2_SUPER_MAGIC};
use crate::image_builder::{read_tar_from_reader, RootfsTree};
use flate2::read::{DeflateDecoder, ZlibDecoder};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};

pub const SQUASHFS_MAGIC: u32 = 0x7371_7368; // "hsqs" LE
pub const UBIFS_NODE_MAGIC: u32 = 0x0610_1831;
pub const UBI_EC_MAGIC: &[u8; 4] = b"UBI#";
pub const UBI_VID_MAGIC: &[u8; 4] = b"UBI!";

const UBIFS_INO_NODE: u8 = 0;
const UBIFS_DATA_NODE: u8 = 1;
const UBIFS_DENT_NODE: u8 = 2;

const UBIFS_COMPR_NONE: u16 = 0;
const UBIFS_COMPR_ZLIB: u16 = 2;

const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const S_IFREG: u16 = 0x8000;
const S_IFLNK: u16 = 0xA000;

fn decompress_zlib_or_deflate(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    if ZlibDecoder::new(input).read_to_end(&mut out).is_ok() && !out.is_empty() {
        return Ok(out);
    }
    out.clear();
    DeflateDecoder::new(input)
        .read_to_end(&mut out)
        .map_err(|e| format!("deflate decompression failed: {}", e))?;
    Ok(out)
}

#[derive(Clone, Debug)]
struct UbifsIno {
    sqnum: u64,
    size: u64,
    nlink: u32,
    uid: u16,
    gid: u16,
    mode: u16,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct UbifsDent {
    sqnum: u64,
    target_inum: u32,
    entry_type: u8,
    name: String,
}

#[derive(Clone, Debug)]
struct UbifsDataBlock {
    sqnum: u64,
    bytes: Vec<u8>,
}

pub fn extract_ubifs_into_tree(raw: &[u8]) -> Result<RootfsTree, String> {
    let mut inodes: BTreeMap<u32, UbifsIno> = BTreeMap::new();
    let mut dents: BTreeMap<(u32, String), UbifsDent> = BTreeMap::new();
    let mut data_blocks: BTreeMap<(u32, u32), UbifsDataBlock> = BTreeMap::new();

    let mut pos = 0usize;
    while pos + 24 <= raw.len() {
        let magic = u32::from_le_bytes([raw[pos], raw[pos + 1], raw[pos + 2], raw[pos + 3]]);
        if magic != UBIFS_NODE_MAGIC {
            pos += 1;
            continue;
        }
        let sqnum = u64::from_le_bytes([
            raw[pos + 8],
            raw[pos + 9],
            raw[pos + 10],
            raw[pos + 11],
            raw[pos + 12],
            raw[pos + 13],
            raw[pos + 14],
            raw[pos + 15],
        ]);
        let node_len =
            u32::from_le_bytes([raw[pos + 16], raw[pos + 17], raw[pos + 18], raw[pos + 19]])
                as usize;
        let node_type = raw[pos + 20];

        if node_len < 24 || pos + node_len > raw.len() {
            pos += 4;
            continue;
        }
        let node = &raw[pos..pos + node_len];

        match node_type {
            UBIFS_INO_NODE if node.len() >= 160 => {
                let inum = u32::from_le_bytes([node[24], node[25], node[26], node[27]]);
                let size = u64::from_le_bytes([
                    node[48], node[49], node[50], node[51], node[52], node[53], node[54], node[55],
                ]);
                let nlink = u32::from_le_bytes([node[92], node[93], node[94], node[95]]);
                let uid = u32::from_le_bytes([node[96], node[97], node[98], node[99]]) as u16;
                let gid = u32::from_le_bytes([node[100], node[101], node[102], node[103]]) as u16;
                let mode = u32::from_le_bytes([node[104], node[105], node[106], node[107]]) as u16;
                let data_len =
                    u32::from_le_bytes([node[112], node[113], node[114], node[115]]) as usize;
                let end = (160 + data_len).min(node.len());
                let inline = node[160..end].to_vec();

                let replace = inodes.get(&inum).map(|old| sqnum >= old.sqnum).unwrap_or(true);
                if replace {
                    inodes.insert(
                        inum,
                        UbifsIno {
                            sqnum,
                            size,
                            nlink,
                            uid,
                            gid,
                            mode,
                            data: inline,
                        },
                    );
                }
            }
            UBIFS_DENT_NODE if node.len() >= 56 => {
                let parent_inum = u32::from_le_bytes([node[24], node[25], node[26], node[27]]);
                let target_inum = u64::from_le_bytes([
                    node[40], node[41], node[42], node[43], node[44], node[45], node[46], node[47],
                ]) as u32;
                let entry_type = node[49];
                let nlen = u16::from_le_bytes([node[50], node[51]]) as usize;
                if 56 + nlen <= node.len() && nlen > 0 {
                    let name = String::from_utf8_lossy(&node[56..56 + nlen])
                        .trim_end_matches('\0')
                        .to_string();
                    if !name.is_empty() && name != "." && name != ".." {
                        let key = (parent_inum, name.clone());
                        let replace = dents.get(&key).map(|old| sqnum >= old.sqnum).unwrap_or(true);
                        if replace {
                            dents.insert(
                                key,
                                UbifsDent {
                                    sqnum,
                                    target_inum,
                                    entry_type,
                                    name,
                                },
                            );
                        }
                    }
                }
            }
            UBIFS_DATA_NODE if node.len() >= 48 => {
                let inum = u32::from_le_bytes([node[24], node[25], node[26], node[27]]);
                let k1 = u32::from_le_bytes([node[28], node[29], node[30], node[31]]);
                let block_num = k1 & 0x1FFF_FFFF;
                let uncomp_len =
                    u32::from_le_bytes([node[40], node[41], node[42], node[43]]) as usize;
                let compr_type = u16::from_le_bytes([node[44], node[45]]);
                let payload = &node[48..];
                let replace = data_blocks
                    .get(&(inum, block_num))
                    .map(|old| sqnum >= old.sqnum)
                    .unwrap_or(true);
                if replace {
                    let decoded = match compr_type {
                        UBIFS_COMPR_NONE => payload[..payload.len().min(uncomp_len)].to_vec(),
                        UBIFS_COMPR_ZLIB => {
                            decompress_zlib_or_deflate(payload).unwrap_or_default()
                        }
                        _ => payload[..payload.len().min(uncomp_len)].to_vec(),
                    };
                    data_blocks.insert(
                        (inum, block_num),
                        UbifsDataBlock {
                            sqnum,
                            bytes: decoded,
                        },
                    );
                }
            }
            _ => {}
        }

        let aligned = (node_len + 7) & !7;
        pos += aligned.max(8);
    }

    if dents.is_empty() && inodes.is_empty() {
        return Err("no valid UBIFS nodes found".to_string());
    }

    let mut children_by_parent: BTreeMap<u32, Vec<UbifsDent>> = BTreeMap::new();
    for ((parent, _), dent) in dents {
        if dent.target_inum != 0 {
            children_by_parent.entry(parent).or_default().push(dent);
        }
    }

    let mut tree = RootfsTree::default();
    let mut stack = vec![(1u32, String::new())];
    while let Some((dir_inum, prefix)) = stack.pop() {
        let Some(children) = children_by_parent.get(&dir_inum) else {
            continue;
        };
        for child in children {
            let rel = if prefix.is_empty() {
                child.name.clone()
            } else {
                format!("{}/{}", prefix, child.name)
            };
            let ino_opt = inodes.get(&child.target_inum);
            if let Some(ino) = ino_opt {
                if ino.nlink == 0 {
                    continue;
                }
            }
            let (fmt, perm, uid, gid) = match ino_opt {
                Some(ino) => (
                    ino.mode & S_IFMT,
                    if (ino.mode & 0o7777) == 0 {
                        0o755
                    } else {
                        ino.mode & 0o7777
                    },
                    ino.uid,
                    ino.gid,
                ),
                None => {
                    let fmt = match child.entry_type {
                        1 => S_IFDIR,
                        2 => S_IFLNK,
                        _ => S_IFREG,
                    };
                    (fmt, 0o755, 0, 0)
                }
            };

            if fmt == S_IFDIR || child.entry_type == 1 {
                tree.insert_dir_with_owner(&rel, perm, uid, gid);
                stack.push((child.target_inum, rel));
            } else if fmt == S_IFLNK || child.entry_type == 2 {
                let target = ino_opt
                    .map(|i| String::from_utf8_lossy(&i.data).to_string())
                    .unwrap_or_default();
                tree.insert_symlink_with_owner(&rel, target, uid, gid);
            } else if fmt == S_IFREG || child.entry_type == 0 {
                let file_size = ino_opt.map(|i| i.size as usize).unwrap_or(0);
                let mut file_bytes = vec![0u8; file_size];
                let num_blks = (file_size + 4095) / 4096;
                for blk_idx in 0..num_blks as u32 {
                    if let Some(blk) = data_blocks.get(&(child.target_inum, blk_idx)) {
                        let start = (blk_idx as usize) * 4096;
                        if start < file_size {
                            let take = blk.bytes.len().min(file_size - start);
                            file_bytes[start..start + take].copy_from_slice(&blk.bytes[..take]);
                        }
                    }
                }
                if file_size == 0 {
                    if let Some(blk) = data_blocks.get(&(child.target_inum, 0)) {
                        file_bytes = blk.bytes.clone();
                    }
                }
                tree.insert_file_with_owner(&rel, file_bytes, perm, uid, gid);
            }
        }
    }

    if tree.entries.is_empty() {
        return Err("UBIFS stream contained no extractable files".to_string());
    }
    Ok(tree)
}

fn read_squashfs_meta_block(raw: &[u8], pos: usize) -> Result<(Vec<u8>, usize), String> {
    if pos + 2 > raw.len() {
        return Err("squashfs metadata block out of bounds".to_string());
    }
    let hdr = u16::from_le_bytes([raw[pos], raw[pos + 1]]);
    let comp_len = (hdr & 0x7FFF) as usize;
    let is_uncompressed = (hdr & 0x8000) != 0;
    if comp_len == 0 || pos + 2 + comp_len > raw.len() {
        return Err("squashfs metadata payload out of bounds".to_string());
    }
    let slice = &raw[pos + 2..pos + 2 + comp_len];
    let data = if is_uncompressed {
        slice.to_vec()
    } else {
        decompress_zlib_or_deflate(slice)?
    };
    Ok((data, pos + 2 + comp_len))
}

fn read_squashfs_meta_stream(
    raw: &[u8],
    table_start: u64,
    block_rel: u64,
    inner_off: usize,
    min_len: usize,
) -> Result<Vec<u8>, String> {
    let mut cur_pos = (table_start + block_rel) as usize;
    let mut stream = Vec::new();
    while stream.len() < inner_off + min_len && cur_pos < raw.len() {
        let Ok((chunk, next_pos)) = read_squashfs_meta_block(raw, cur_pos) else {
            break;
        };
        stream.extend_from_slice(&chunk);
        cur_pos = next_pos;
    }
    if stream.len() < inner_off {
        return Err("squashfs metadata inner_off exceeds stream".to_string());
    }
    Ok(stream[inner_off..].to_vec())
}

fn decompress_squashfs_data_block(
    raw: &[u8],
    offset: usize,
    size_hdr: u32,
    block_size: usize,
) -> Result<Vec<u8>, String> {
    if size_hdr == 0 {
        return Ok(vec![0u8; block_size]);
    }
    let comp_len = (size_hdr & 0x00FF_FFFF) as usize;
    let is_uncompressed = (size_hdr & 0x0100_0000) != 0;
    if offset + comp_len > raw.len() {
        return Err("squashfs data block out of bounds".to_string());
    }
    let slice = &raw[offset..offset + comp_len];
    if is_uncompressed {
        Ok(slice.to_vec())
    } else {
        decompress_zlib_or_deflate(slice)
    }
}

pub fn extract_squashfs_into_tree(raw: &[u8]) -> Result<RootfsTree, String> {
    if raw.len() < 96 {
        return Err("squashfs image too small (< 96 bytes)".to_string());
    }
    let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if magic != SQUASHFS_MAGIC {
        return Err(format!("invalid squashfs magic: 0x{:08x}", magic));
    }

    let block_size = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]) as usize;
    let fragments_count = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;
    let no_ids = u16::from_le_bytes([raw[26], raw[27]]) as usize;
    let root_inode_ref = u64::from_le_bytes([
        raw[32], raw[33], raw[34], raw[35], raw[36], raw[37], raw[38], raw[39],
    ]);
    let id_table_start = u64::from_le_bytes([
        raw[48], raw[49], raw[50], raw[51], raw[52], raw[53], raw[54], raw[55],
    ]);
    let inode_table_start = u64::from_le_bytes([
        raw[64], raw[65], raw[66], raw[67], raw[68], raw[69], raw[70], raw[71],
    ]);
    let directory_table_start = u64::from_le_bytes([
        raw[72], raw[73], raw[74], raw[75], raw[76], raw[77], raw[78], raw[79],
    ]);
    let fragment_table_start = u64::from_le_bytes([
        raw[80], raw[81], raw[82], raw[83], raw[84], raw[85], raw[86], raw[87],
    ]);

    // 1. Parse ID table (if present)
    let mut id_table: Vec<u16> = Vec::new();
    if no_ids > 0 && id_table_start != u64::MAX && (id_table_start as usize) + 8 <= raw.len() {
        let id_blocks = (no_ids + 2047) / 2048;
        for b in 0..id_blocks {
            let ptr_off = (id_table_start as usize) + b * 8;
            if ptr_off + 8 > raw.len() {
                break;
            }
            let meta_off = u64::from_le_bytes([
                raw[ptr_off],
                raw[ptr_off + 1],
                raw[ptr_off + 2],
                raw[ptr_off + 3],
                raw[ptr_off + 4],
                raw[ptr_off + 5],
                raw[ptr_off + 6],
                raw[ptr_off + 7],
            ]) as usize;
            if let Ok((meta_bytes, _)) = read_squashfs_meta_block(raw, meta_off) {
                for chunk in meta_bytes.chunks_exact(4) {
                    if id_table.len() < no_ids {
                        let id =
                            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u16;
                        id_table.push(id);
                    }
                }
            }
        }
    }

    // 2. Parse Fragment table (if present)
    let mut fragments: Vec<(u64, u32)> = Vec::new();
    if fragments_count > 0
        && fragment_table_start != u64::MAX
        && (fragment_table_start as usize) + 8 <= raw.len()
    {
        let frag_blocks = (fragments_count + 511) / 512;
        for b in 0..frag_blocks {
            let ptr_off = (fragment_table_start as usize) + b * 8;
            if ptr_off + 8 > raw.len() {
                break;
            }
            let meta_off = u64::from_le_bytes([
                raw[ptr_off],
                raw[ptr_off + 1],
                raw[ptr_off + 2],
                raw[ptr_off + 3],
                raw[ptr_off + 4],
                raw[ptr_off + 5],
                raw[ptr_off + 6],
                raw[ptr_off + 7],
            ]) as usize;
            if let Ok((meta_bytes, _)) = read_squashfs_meta_block(raw, meta_off) {
                for chunk in meta_bytes.chunks_exact(16) {
                    if fragments.len() < fragments_count {
                        let start_blk = u64::from_le_bytes([
                            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                            chunk[7],
                        ]);
                        let sz = u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
                        fragments.push((start_blk, sz));
                    }
                }
            }
        }
    }

    let resolve_id = |idx: u16| -> u16 { id_table.get(idx as usize).copied().unwrap_or(0) };

    let mut tree = RootfsTree::default();
    let mut stack = vec![(root_inode_ref, String::new())];

    while let Some((inode_ref, prefix)) = stack.pop() {
        let block_rel = inode_ref >> 16;
        let inner_off = (inode_ref & 0xFFFF) as usize;
        let ino_buf = read_squashfs_meta_stream(raw, inode_table_start, block_rel, inner_off, 64)?;
        if ino_buf.len() < 16 {
            continue;
        }
        let inode_type = u16::from_le_bytes([ino_buf[0], ino_buf[1]]);
        let mode = u16::from_le_bytes([ino_buf[2], ino_buf[3]]) & 0o7777;
        let uid = resolve_id(u16::from_le_bytes([ino_buf[4], ino_buf[5]]));
        let gid = resolve_id(u16::from_le_bytes([ino_buf[6], ino_buf[7]]));

        match inode_type {
            // Basic Directory (1) or Extended Directory (8)
            1 | 8 => {
                let (dir_block_start, dir_offset, file_size) = if inode_type == 1 {
                    if ino_buf.len() < 32 {
                        continue;
                    }
                    let dbs =
                        u32::from_le_bytes([ino_buf[16], ino_buf[17], ino_buf[18], ino_buf[19]])
                            as u64;
                    let fsz = u16::from_le_bytes([ino_buf[24], ino_buf[25]]) as usize;
                    let doff = u16::from_le_bytes([ino_buf[26], ino_buf[27]]) as usize;
                    (dbs, doff, fsz)
                } else {
                    if ino_buf.len() < 40 {
                        continue;
                    }
                    let fsz =
                        u32::from_le_bytes([ino_buf[20], ino_buf[21], ino_buf[22], ino_buf[23]])
                            as usize;
                    let dbs =
                        u32::from_le_bytes([ino_buf[24], ino_buf[25], ino_buf[26], ino_buf[27]])
                            as u64;
                    let doff = u16::from_le_bytes([ino_buf[34], ino_buf[35]]) as usize;
                    (dbs, doff, fsz)
                };

                if !prefix.is_empty() {
                    tree.insert_dir_with_owner(&prefix, mode, uid, gid);
                }
                let dir_bytes_len = file_size.saturating_sub(3);
                if dir_bytes_len == 0 {
                    continue;
                }
                let dir_stream = read_squashfs_meta_stream(
                    raw,
                    directory_table_start,
                    dir_block_start,
                    dir_offset,
                    dir_bytes_len,
                )?;
                let dir_slice = &dir_stream[..dir_bytes_len.min(dir_stream.len())];
                let mut pos = 0usize;
                while pos + 12 <= dir_slice.len() {
                    let count = u32::from_le_bytes([
                        dir_slice[pos],
                        dir_slice[pos + 1],
                        dir_slice[pos + 2],
                        dir_slice[pos + 3],
                    ]) as usize
                        + 1;
                    let start_blk = u32::from_le_bytes([
                        dir_slice[pos + 4],
                        dir_slice[pos + 5],
                        dir_slice[pos + 6],
                        dir_slice[pos + 7],
                    ]) as u64;
                    pos += 12;
                    for _ in 0..count {
                        if pos + 8 > dir_slice.len() {
                            break;
                        }
                        let entry_off =
                            u16::from_le_bytes([dir_slice[pos], dir_slice[pos + 1]]) as u64;
                        let name_size =
                            u16::from_le_bytes([dir_slice[pos + 6], dir_slice[pos + 7]]) as usize
                                + 1;
                        pos += 8;
                        if pos + name_size > dir_slice.len() {
                            break;
                        }
                        let name =
                            String::from_utf8_lossy(&dir_slice[pos..pos + name_size]).to_string();
                        pos += name_size;
                        let child_path = if prefix.is_empty() {
                            name
                        } else {
                            format!("{}/{}", prefix, name)
                        };
                        let child_ref = (start_blk << 16) | entry_off;
                        stack.push((child_ref, child_path));
                    }
                }
            }
            // Basic Regular File (2) or Extended Regular File (9)
            2 | 9 => {
                let (blocks_start, frag_idx, frag_off, file_size, hdr_len) = if inode_type == 2 {
                    if ino_buf.len() < 32 {
                        continue;
                    }
                    let bs =
                        u32::from_le_bytes([ino_buf[16], ino_buf[17], ino_buf[18], ino_buf[19]])
                            as usize;
                    let fi =
                        u32::from_le_bytes([ino_buf[20], ino_buf[21], ino_buf[22], ino_buf[23]]);
                    let fo =
                        u32::from_le_bytes([ino_buf[24], ino_buf[25], ino_buf[26], ino_buf[27]])
                            as usize;
                    let fsz =
                        u32::from_le_bytes([ino_buf[28], ino_buf[29], ino_buf[30], ino_buf[31]])
                            as usize;
                    (bs, fi, fo, fsz, 32usize)
                } else {
                    if ino_buf.len() < 56 {
                        continue;
                    }
                    let bs = u64::from_le_bytes([
                        ino_buf[16],
                        ino_buf[17],
                        ino_buf[18],
                        ino_buf[19],
                        ino_buf[20],
                        ino_buf[21],
                        ino_buf[22],
                        ino_buf[23],
                    ]) as usize;
                    let fsz = u64::from_le_bytes([
                        ino_buf[24],
                        ino_buf[25],
                        ino_buf[26],
                        ino_buf[27],
                        ino_buf[28],
                        ino_buf[29],
                        ino_buf[30],
                        ino_buf[31],
                    ]) as usize;
                    let fi =
                        u32::from_le_bytes([ino_buf[44], ino_buf[45], ino_buf[46], ino_buf[47]]);
                    let fo =
                        u32::from_le_bytes([ino_buf[48], ino_buf[49], ino_buf[50], ino_buf[51]])
                            as usize;
                    (bs, fi, fo, fsz, 56usize)
                };

                let num_full_blocks = if frag_idx == u32::MAX {
                    (file_size + block_size.max(1) - 1) / block_size.max(1)
                } else {
                    file_size / block_size.max(1)
                };
                let full_ino_buf = read_squashfs_meta_stream(
                    raw,
                    inode_table_start,
                    block_rel,
                    inner_off,
                    hdr_len + num_full_blocks * 4,
                )?;
                let mut file_data = Vec::with_capacity(file_size);
                let mut cur_data_off = blocks_start;
                for b in 0..num_full_blocks {
                    let sz_off = hdr_len + b * 4;
                    if sz_off + 4 > full_ino_buf.len() {
                        break;
                    }
                    let b_sz = u32::from_le_bytes([
                        full_ino_buf[sz_off],
                        full_ino_buf[sz_off + 1],
                        full_ino_buf[sz_off + 2],
                        full_ino_buf[sz_off + 3],
                    ]);
                    let dec = decompress_squashfs_data_block(raw, cur_data_off, b_sz, block_size)?;
                    let take = dec.len().min(file_size.saturating_sub(file_data.len()));
                    file_data.extend_from_slice(&dec[..take]);
                    cur_data_off += (b_sz & 0x00FF_FFFF) as usize;
                }

                if frag_idx != u32::MAX && file_data.len() < file_size {
                    if let Some(&(frag_start, frag_sz)) = fragments.get(frag_idx as usize) {
                        let frag_bytes = decompress_squashfs_data_block(
                            raw,
                            frag_start as usize,
                            frag_sz,
                            block_size,
                        )?;
                        let rem = file_size - file_data.len();
                        if frag_off + rem <= frag_bytes.len() {
                            file_data.extend_from_slice(&frag_bytes[frag_off..frag_off + rem]);
                        }
                    }
                }
                tree.insert_file_with_owner(&prefix, file_data, mode, uid, gid);
            }
            // Basic Symlink (3) or Extended Symlink (10)
            3 | 10 => {
                if ino_buf.len() < 24 {
                    continue;
                }
                let target_len =
                    u32::from_le_bytes([ino_buf[20], ino_buf[21], ino_buf[22], ino_buf[23]])
                        as usize;
                let full_ino_buf = read_squashfs_meta_stream(
                    raw,
                    inode_table_start,
                    block_rel,
                    inner_off,
                    24 + target_len,
                )?;
                if 24 + target_len <= full_ino_buf.len() {
                    let target =
                        String::from_utf8_lossy(&full_ino_buf[24..24 + target_len]).to_string();
                    tree.insert_symlink_with_owner(&prefix, target, uid, gid);
                }
            }
            _ => {}
        }
    }

    Ok(tree)
}

fn detect_ubi_peb_size(raw: &[u8], first_data_offset: usize) -> usize {
    let min_step = first_data_offset.max(512);
    let mut off = min_step;
    while off + 64 <= raw.len() {
        if &raw[off..off + 4] == UBI_EC_MAGIC && raw[off + 4] == 1 {
            let vid_off =
                u32::from_be_bytes([raw[off + 16], raw[off + 17], raw[off + 18], raw[off + 19]])
                    as usize;
            let data_off =
                u32::from_be_bytes([raw[off + 20], raw[off + 21], raw[off + 22], raw[off + 23]])
                    as usize;
            if vid_off >= 64 && data_off > vid_off && off + data_off <= raw.len() {
                return off;
            }
        }
        off += 64;
    }
    raw.len()
}

pub fn extract_payload_bytes_into_tree(payload: &[u8]) -> Result<RootfsTree, String> {
    if payload.len() >= 4 {
        let magic_le = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        if magic_le == SQUASHFS_MAGIC {
            return extract_squashfs_into_tree(payload);
        }
        if magic_le == UBIFS_NODE_MAGIC {
            return extract_ubifs_into_tree(payload);
        }
    }
    if payload.len() >= 2048
        && u16::from_le_bytes([payload[1080], payload[1081]]) == EXT2_SUPER_MAGIC
    {
        let tmp = std::env::temp_dir().join(format!(
            "kuyase-ubi-ext-{}-{}.img",
            std::process::id(),
            payload.len()
        ));
        fs::write(&tmp, payload).map_err(|e| e.to_string())?;
        let res = read_ext2_image(&tmp);
        let _ = fs::remove_file(&tmp);
        return res;
    }
    if payload.windows(4).any(|w| w == [0x31, 0x18, 0x10, 0x06]) {
        if let Ok(tree) = extract_ubifs_into_tree(payload) {
            return Ok(tree);
        }
    }
    if let Some(pos) = payload.windows(4).position(|w| w == b"hsqs") {
        if let Ok(tree) = extract_squashfs_into_tree(&payload[pos..]) {
            return Ok(tree);
        }
    }
    if let Ok(t) = read_tar_from_reader(Cursor::new(payload)) {
        if !t.entries.is_empty() {
            return Ok(t);
        }
    }
    Err("unrecognized volume payload inside UBI image".to_string())
}

pub fn extract_ubi_into_tree(raw: &[u8]) -> Result<RootfsTree, String> {
    if raw.len() < 128 || &raw[0..4] != UBI_EC_MAGIC {
        return Err("not a valid UBI# image".to_string());
    }
    let first_data_off = u32::from_be_bytes([raw[20], raw[21], raw[22], raw[23]]) as usize;
    let peb_size = detect_ubi_peb_size(raw, first_data_off);
    if peb_size < 128 {
        return Err("invalid UBI PEB size".to_string());
    }

    // Map (vol_id, lnum) -> (sqnum, slice)
    let mut volumes: BTreeMap<u32, BTreeMap<u32, (u64, &[u8])>> = BTreeMap::new();
    let mut peb_off = 0usize;
    while peb_off + 64 <= raw.len() {
        let peb = &raw[peb_off..(peb_off + peb_size).min(raw.len())];
        if &peb[0..4] == UBI_EC_MAGIC {
            let vid_off = u32::from_be_bytes([peb[16], peb[17], peb[18], peb[19]]) as usize;
            let data_off = u32::from_be_bytes([peb[20], peb[21], peb[22], peb[23]]) as usize;
            if vid_off + 64 <= peb.len() && data_off <= peb.len() {
                let vid = &peb[vid_off..vid_off + 64];
                if &vid[0..4] == UBI_VID_MAGIC {
                    let vol_type = vid[5];
                    let vol_id = u32::from_be_bytes([vid[8], vid[9], vid[10], vid[11]]);
                    let lnum = u32::from_be_bytes([vid[12], vid[13], vid[14], vid[15]]);
                    let data_size =
                        u32::from_be_bytes([vid[20], vid[21], vid[22], vid[23]]) as usize;
                    let sqnum = u64::from_be_bytes([
                        vid[36], vid[37], vid[38], vid[39], vid[40], vid[41], vid[42], vid[43],
                    ]);

                    if vol_id < 0x7FFF_EFFF {
                        let leb_cap = peb.len() - data_off;
                        let take = if vol_type == 2 && data_size > 0 && data_size <= leb_cap {
                            data_size
                        } else {
                            leb_cap
                        };
                        let data_slice = &peb[data_off..data_off + take];
                        let vol_map = volumes.entry(vol_id).or_default();
                        let replace = vol_map
                            .get(&lnum)
                            .map(|(old_sq, _)| sqnum >= *old_sq)
                            .unwrap_or(true);
                        if replace {
                            vol_map.insert(lnum, (sqnum, data_slice));
                        }
                    }
                }
            }
        }
        peb_off += peb_size;
    }

    for (_vol_id, lebs) in volumes {
        let mut reassembled = Vec::new();
        for (_lnum, (_sq, bytes)) in lebs {
            reassembled.extend_from_slice(bytes);
        }
        if let Ok(tree) = extract_payload_bytes_into_tree(&reassembled) {
            if !tree.entries.is_empty() {
                return Ok(tree);
            }
        }
    }

    // Fallback: scan raw image directly
    extract_payload_bytes_into_tree(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_builder::RootfsNodeKind;

    fn build_ubi_peb(
        peb_size: usize,
        vol_id: u32,
        lnum: u32,
        sqnum: u64,
        leb_payload: &[u8],
    ) -> Vec<u8> {
        let vid_off = 64u32;
        let data_off = 128u32;
        let mut peb = vec![0xFFu8; peb_size];
        // EC header
        peb[0..4].copy_from_slice(UBI_EC_MAGIC);
        peb[4] = 1;
        peb[16..20].copy_from_slice(&vid_off.to_be_bytes());
        peb[20..24].copy_from_slice(&data_off.to_be_bytes());
        // VID header
        let vid = &mut peb[vid_off as usize..vid_off as usize + 64];
        vid.fill(0);
        vid[0..4].copy_from_slice(UBI_VID_MAGIC);
        vid[4] = 1;
        vid[5] = 1; // dynamic volume
        vid[8..12].copy_from_slice(&vol_id.to_be_bytes());
        vid[12..16].copy_from_slice(&lnum.to_be_bytes());
        vid[20..24].copy_from_slice(&(leb_payload.len() as u32).to_be_bytes());
        vid[36..44].copy_from_slice(&sqnum.to_be_bytes());
        // LEB data
        peb[data_off as usize..data_off as usize + leb_payload.len()].copy_from_slice(leb_payload);
        peb
    }

    fn build_ubifs_ino_node(
        sqnum: u64,
        inum: u32,
        mode: u16,
        uid: u16,
        gid: u16,
        size: u64,
        inline_data: &[u8],
    ) -> Vec<u8> {
        let len = 160 + inline_data.len();
        let mut buf = vec![0u8; (len + 7) & !7];
        buf[0..4].copy_from_slice(&UBIFS_NODE_MAGIC.to_le_bytes());
        buf[8..16].copy_from_slice(&sqnum.to_le_bytes());
        buf[16..20].copy_from_slice(&(len as u32).to_le_bytes());
        buf[20] = UBIFS_INO_NODE;
        buf[24..28].copy_from_slice(&inum.to_le_bytes());
        buf[48..56].copy_from_slice(&size.to_le_bytes());
        buf[92..96].copy_from_slice(&1u32.to_le_bytes()); // nlink = 1
        buf[96..100].copy_from_slice(&(uid as u32).to_le_bytes());
        buf[100..104].copy_from_slice(&(gid as u32).to_le_bytes());
        buf[104..108].copy_from_slice(&(mode as u32).to_le_bytes());
        buf[112..116].copy_from_slice(&(inline_data.len() as u32).to_le_bytes());
        buf[160..160 + inline_data.len()].copy_from_slice(inline_data);
        buf
    }

    fn build_ubifs_dent_node(
        sqnum: u64,
        parent_inum: u32,
        target_inum: u32,
        entry_type: u8,
        name: &str,
    ) -> Vec<u8> {
        let nb = name.as_bytes();
        let len = 56 + nb.len() + 1;
        let mut buf = vec![0u8; (len + 7) & !7];
        buf[0..4].copy_from_slice(&UBIFS_NODE_MAGIC.to_le_bytes());
        buf[8..16].copy_from_slice(&sqnum.to_le_bytes());
        buf[16..20].copy_from_slice(&(len as u32).to_le_bytes());
        buf[20] = UBIFS_DENT_NODE;
        buf[24..28].copy_from_slice(&parent_inum.to_le_bytes());
        buf[40..48].copy_from_slice(&(target_inum as u64).to_le_bytes());
        buf[49] = entry_type;
        buf[50..52].copy_from_slice(&(nb.len() as u16).to_le_bytes());
        buf[56..56 + nb.len()].copy_from_slice(nb);
        buf
    }

    fn build_ubifs_data_node(sqnum: u64, inum: u32, block_num: u32, data: &[u8]) -> Vec<u8> {
        let len = 48 + data.len();
        let mut buf = vec![0u8; (len + 7) & !7];
        buf[0..4].copy_from_slice(&UBIFS_NODE_MAGIC.to_le_bytes());
        buf[8..16].copy_from_slice(&sqnum.to_le_bytes());
        buf[16..20].copy_from_slice(&(len as u32).to_le_bytes());
        buf[20] = UBIFS_DATA_NODE;
        buf[24..28].copy_from_slice(&inum.to_le_bytes());
        let k1 = (1u32 << 29) | (block_num & 0x1FFF_FFFF);
        buf[28..32].copy_from_slice(&k1.to_le_bytes());
        buf[40..44].copy_from_slice(&(data.len() as u32).to_le_bytes());
        buf[44..46].copy_from_slice(&UBIFS_COMPR_NONE.to_le_bytes());
        buf[48..48 + data.len()].copy_from_slice(data);
        buf
    }

    #[test]
    fn test_extract_synthetic_ubi_ubifs_image() {
        let mut leb0 = Vec::new();
        // Root dir (inum 1), /bin dir (inum 2), /bin/busybox file (inum 3), /bin/sh symlink (inum 4)
        leb0.extend_from_slice(&build_ubifs_ino_node(1, 1, S_IFDIR | 0o755, 0, 0, 0, b""));
        leb0.extend_from_slice(&build_ubifs_dent_node(2, 1, 2, 1, "bin"));
        leb0.extend_from_slice(&build_ubifs_ino_node(3, 2, S_IFDIR | 0o755, 0, 0, 0, b""));

        let mut leb1 = Vec::new();
        let busybox_bytes = b"\x7fELF-ubifs-busybox";
        leb1.extend_from_slice(&build_ubifs_dent_node(4, 2, 3, 0, "busybox"));
        leb1.extend_from_slice(&build_ubifs_ino_node(
            5,
            3,
            S_IFREG | 0o755,
            1000,
            1000,
            busybox_bytes.len() as u64,
            b"",
        ));
        leb1.extend_from_slice(&build_ubifs_data_node(6, 3, 0, busybox_bytes));
        leb1.extend_from_slice(&build_ubifs_dent_node(7, 2, 4, 2, "sh"));
        leb1.extend_from_slice(&build_ubifs_ino_node(
            8,
            4,
            S_IFLNK | 0o777,
            0,
            0,
            7,
            b"busybox",
        ));

        // Put LEB 1 in PEB 0 and LEB 0 in PEB 1 out-of-order to verify UBI (vol_id, lnum) sorting!
        let mut ubi_img = Vec::new();
        ubi_img.extend_from_slice(&build_ubi_peb(2048, 0, 1, 20, &leb1));
        ubi_img.extend_from_slice(&build_ubi_peb(2048, 0, 0, 10, &leb0));

        let tree = extract_ubi_into_tree(&ubi_img).unwrap();
        let bb = tree.get("bin/busybox").unwrap();
        assert_eq!(bb.mode, 0o755);
        assert_eq!(bb.uid, 1000);
        assert_eq!(bb.gid, 1000);
        assert_eq!(tree.get_file_bytes("bin/busybox").unwrap(), busybox_bytes);
        assert_eq!(
            tree.get("bin/sh").unwrap().kind,
            RootfsNodeKind::Symlink("busybox".to_string())
        );
    }

    #[test]
    fn test_extract_synthetic_ubi_squashfs_image() {
        // Build a valid uncompressed SquashFS 4.0 image with root dir (ino 1) -> file "hello.txt" (ino 2)
        let file_content = b"squashfs-inside-ubi-payload";
        let mut sqsh = vec![0u8; 512];

        // Data block for hello.txt at offset 96
        let data_start = 96usize;
        sqsh[data_start..data_start + file_content.len()].copy_from_slice(file_content);

        // Inode table metadata block at offset 160 (uncompressed flag 0x8000)
        // Inode 2 (basic file, 36 bytes) at inner_off 0; Inode 1 (basic dir, 32 bytes) at inner_off 36
        let inode_table_start = 160u64;
        let mut ino_meta = Vec::new();
        // Inode 2: basic file (type = 2)
        ino_meta.extend_from_slice(&2u16.to_le_bytes()); // inode_type = 2
        ino_meta.extend_from_slice(&0o644u16.to_le_bytes()); // mode
        ino_meta.extend_from_slice(&0u16.to_le_bytes()); // uid_idx = 0
        ino_meta.extend_from_slice(&0u16.to_le_bytes()); // gid_idx = 0
        ino_meta.extend_from_slice(&0u32.to_le_bytes()); // mtime
        ino_meta.extend_from_slice(&2u32.to_le_bytes()); // ino = 2
        ino_meta.extend_from_slice(&(data_start as u32).to_le_bytes()); // blocks_start
        ino_meta.extend_from_slice(&u32::MAX.to_le_bytes()); // fragment_block_index = NONE
        ino_meta.extend_from_slice(&0u32.to_le_bytes()); // block_offset
        ino_meta.extend_from_slice(&(file_content.len() as u32).to_le_bytes()); // file_size
        let blk_sz_hdr = (file_content.len() as u32) | 0x0100_0000; // uncompressed block
        ino_meta.extend_from_slice(&blk_sz_hdr.to_le_bytes());
        assert_eq!(ino_meta.len(), 36);

        // Directory table entry for "hello.txt" -> 12-byte header + 8-byte entry + 9-byte name = 29 bytes
        let mut dir_meta = Vec::new();
        dir_meta.extend_from_slice(&0u32.to_le_bytes()); // count - 1 = 0 (1 entry)
        dir_meta.extend_from_slice(&0u32.to_le_bytes()); // start_block = 0
        dir_meta.extend_from_slice(&1u32.to_le_bytes()); // inode_number = 1
        dir_meta.extend_from_slice(&0u16.to_le_bytes()); // entry_offset = 0 (points to Inode 2)
        dir_meta.extend_from_slice(&1i16.to_le_bytes()); // inode_offset = +1
        dir_meta.extend_from_slice(&2u16.to_le_bytes()); // type = file
        dir_meta.extend_from_slice(&8u16.to_le_bytes()); // name_size - 1 = 8 ("hello.txt" len 9)
        dir_meta.extend_from_slice(b"hello.txt");

        // Inode 1: basic dir (type = 1) at inner_off 36
        let root_inner_off = ino_meta.len() as u64;
        ino_meta.extend_from_slice(&1u16.to_le_bytes()); // inode_type = 1
        ino_meta.extend_from_slice(&0o755u16.to_le_bytes()); // mode
        ino_meta.extend_from_slice(&0u16.to_le_bytes());
        ino_meta.extend_from_slice(&0u16.to_le_bytes());
        ino_meta.extend_from_slice(&0u32.to_le_bytes());
        ino_meta.extend_from_slice(&1u32.to_le_bytes()); // ino = 1
        ino_meta.extend_from_slice(&0u32.to_le_bytes()); // dir_block_start = 0
        ino_meta.extend_from_slice(&2u32.to_le_bytes()); // nlink = 2
        ino_meta.extend_from_slice(&((dir_meta.len() + 3) as u16).to_le_bytes()); // file_size
        ino_meta.extend_from_slice(&0u16.to_le_bytes()); // offset = 0
        ino_meta.extend_from_slice(&1u32.to_le_bytes()); // parent_inode = 1

        let ino_hdr = (ino_meta.len() as u16) | 0x8000;
        sqsh[inode_table_start as usize..inode_table_start as usize + 2]
            .copy_from_slice(&ino_hdr.to_le_bytes());
        sqsh[inode_table_start as usize + 2..inode_table_start as usize + 2 + ino_meta.len()]
            .copy_from_slice(&ino_meta);

        let directory_table_start = (inode_table_start as usize + 2 + ino_meta.len()) as u64;
        let dir_hdr = (dir_meta.len() as u16) | 0x8000;
        sqsh[directory_table_start as usize..directory_table_start as usize + 2]
            .copy_from_slice(&dir_hdr.to_le_bytes());
        sqsh[directory_table_start as usize + 2
            ..directory_table_start as usize + 2 + dir_meta.len()]
            .copy_from_slice(&dir_meta);

        // Superblock at 0..96
        sqsh[0..4].copy_from_slice(&SQUASHFS_MAGIC.to_le_bytes());
        sqsh[4..8].copy_from_slice(&2u32.to_le_bytes()); // inodes = 2
        sqsh[12..16].copy_from_slice(&4096u32.to_le_bytes()); // block_size = 4096
        sqsh[20..22].copy_from_slice(&1u16.to_le_bytes()); // compression = 1 (gzip)
        sqsh[22..24].copy_from_slice(&12u16.to_le_bytes()); // block_log = 12
        sqsh[28..30].copy_from_slice(&4u16.to_le_bytes()); // major = 4
        sqsh[30..32].copy_from_slice(&0u16.to_le_bytes()); // minor = 0
        sqsh[32..40].copy_from_slice(&root_inner_off.to_le_bytes()); // root_inode
        sqsh[48..56].copy_from_slice(&u64::MAX.to_le_bytes()); // id_table_start
        sqsh[64..72].copy_from_slice(&inode_table_start.to_le_bytes());
        sqsh[72..80].copy_from_slice(&directory_table_start.to_le_bytes());
        sqsh[80..88].copy_from_slice(&u64::MAX.to_le_bytes()); // fragment_table_start

        let ubi_sqsh = build_ubi_peb(1024, 0, 0, 1, &sqsh);
        let tree = extract_ubi_into_tree(&ubi_sqsh).unwrap();
        assert_eq!(tree.get_file_bytes("hello.txt").unwrap(), file_content);
        assert_eq!(tree.get("hello.txt").unwrap().mode, 0o644);
    }
}
