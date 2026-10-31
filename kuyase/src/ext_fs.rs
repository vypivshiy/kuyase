use crate::image_builder::{RootfsNodeKind, RootfsTree};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const BLOCK_SIZE: usize = 4096;
pub const EXT2_SUPER_MAGIC: u16 = 0xEF53;
pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
pub const EXT4_EXTENT_MAGIC: u16 = 0xF30A;
pub const EXT4_FEATURE_INCOMPAT_64BIT: u32 = 0x0080;
const INODE_SIZE: usize = 128;
const BLOCKS_PER_GROUP: u32 = 32768;
const INODES_PER_GROUP: u32 = 2048;

const S_IFLNK: u16 = 0xA000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;

const FT_REG_FILE: u8 = 1;
const FT_DIR: u8 = 2;
const FT_SYMLINK: u8 = 7;

#[derive(Clone, Debug)]
struct InodeSpec {
    ino: u32,
    parent_ino: u32,
    mode: u16,
    uid: u16,
    gid: u16,
    file_type: u8,
    data: Vec<u8>,
    symlink: Option<String>,
    children: Vec<(String, u32, u8)>,
}

fn set_bit(bitmap: &mut [u8], bit: usize) {
    bitmap[bit / 8] |= 1u8 << (bit % 8);
}

fn encode_dir_blocks(ino: u32, parent_ino: u32, children: &[(String, u32, u8)]) -> Vec<u8> {
    let mut entries: Vec<(u32, u8, &[u8])> = Vec::with_capacity(2 + children.len());
    entries.push((ino, FT_DIR, b"."));
    entries.push((parent_ino, FT_DIR, b".."));
    for (name, c_ino, ft) in children {
        entries.push((*c_ino, *ft, name.as_bytes()));
    }

    let mut out = Vec::new();
    let mut blk = vec![0u8; BLOCK_SIZE];
    let mut off = 0usize;
    let mut prev_off = 0usize;

    for (e_ino, ft, name) in entries {
        let name_len = name.len().min(255);
        let needed = (8 + name_len + 3) & !3;
        if off + needed > BLOCK_SIZE {
            let rem = (BLOCK_SIZE - prev_off) as u16;
            blk[prev_off + 4..prev_off + 6].copy_from_slice(&rem.to_le_bytes());
            out.extend_from_slice(&blk);
            blk.fill(0);
            off = 0;
        }
        prev_off = off;
        blk[off..off + 4].copy_from_slice(&e_ino.to_le_bytes());
        blk[off + 4..off + 6].copy_from_slice(&(needed as u16).to_le_bytes());
        blk[off + 6] = name_len as u8;
        blk[off + 7] = ft;
        blk[off + 8..off + 8 + name_len].copy_from_slice(&name[..name_len]);
        off += needed;
    }

    let rem = (BLOCK_SIZE - prev_off) as u16;
    blk[prev_off + 4..prev_off + 6].copy_from_slice(&rem.to_le_bytes());
    out.extend_from_slice(&blk);
    out
}

pub fn write_ext2_image(
    tree: &RootfsTree,
    out_path: &Path,
    requested_size_bytes: u64,
) -> Result<(), String> {
    // 1. Expand tree so all parent directories exist
    let mut full = tree.clone();
    let paths: Vec<String> = full.entries.keys().cloned().collect();
    for p in paths {
        let mut cur = String::new();
        let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(part);
            if full.get(&cur).is_none() {
                full.insert_dir(&cur, 0o755);
            }
        }
    }

    // 2. Assign inode numbers (2 = root, 11+ = entries)
    let mut path_to_ino: BTreeMap<String, u32> = BTreeMap::new();
    path_to_ino.insert(String::new(), 2);
    let mut next_ino = 11u32;
    for path in full.entries.keys() {
        path_to_ino.insert(path.clone(), next_ino);
        next_ino += 1;
    }

    let mut specs: BTreeMap<u32, InodeSpec> = BTreeMap::new();
    specs.insert(
        2,
        InodeSpec {
            ino: 2,
            parent_ino: 2,
            mode: S_IFDIR | 0o755,
            uid: 0,
            gid: 0,
            file_type: FT_DIR,
            data: Vec::new(),
            symlink: None,
            children: Vec::new(),
        },
    );

    for (path, node) in &full.entries {
        let ino = path_to_ino[path];
        let (parent_path, base_name) = match path.rfind('/') {
            Some(idx) => (&path[..idx], &path[idx + 1..]),
            None => ("", path.as_str()),
        };
        let parent_ino = *path_to_ino.get(parent_path).unwrap_or(&2);
        let (mode, ft, data, symlink) = match &node.kind {
            RootfsNodeKind::Dir => (S_IFDIR | (node.mode & 0o7777), FT_DIR, Vec::new(), None),
            RootfsNodeKind::File(bytes) => {
                (S_IFREG | (node.mode & 0o7777), FT_REG_FILE, bytes.clone(), None)
            }
            RootfsNodeKind::Symlink(target) => {
                (S_IFLNK | 0o777, FT_SYMLINK, Vec::new(), Some(target.clone()))
            }
        };

        specs.insert(
            ino,
            InodeSpec {
                ino,
                parent_ino,
                mode,
                uid: node.uid,
                gid: node.gid,
                file_type: ft,
                data,
                symlink,
                children: Vec::new(),
            },
        );
        if let Some(parent) = specs.get_mut(&parent_ino) {
            parent.children.push((base_name.to_string(), ino, ft));
        }
    }

    // 3. Calculate required size and layout
    let payload_blocks: usize = specs
        .values()
        .map(|s| (s.data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE + 2)
        .sum();
    let min_blocks = (payload_blocks + 2048).max(4096);
    let req_blocks = (requested_size_bytes as usize) / BLOCK_SIZE;
    let total_blocks = req_blocks.max(min_blocks) as u32;
    let num_groups = (total_blocks + BLOCKS_PER_GROUP - 1) / BLOCKS_PER_GROUP;
    let total_inodes = ((next_ino + 128).max(num_groups * INODES_PER_GROUP) + num_groups - 1)
        / num_groups
        * num_groups;
    let inodes_per_group = total_inodes / num_groups;
    let inode_table_blocks = (inodes_per_group as usize * INODE_SIZE + BLOCK_SIZE - 1) / BLOCK_SIZE;
    let gdt_blocks = ((num_groups as usize * 32) + BLOCK_SIZE - 1) / BLOCK_SIZE;

    // Group 0 metadata: block 0 (superblock), blocks 1..1+gdt_blocks (GDT), block BBM, block IBM, inode table
    let mut bbm_blk = vec![0u32; num_groups as usize];
    let mut ibm_blk = vec![0u32; num_groups as usize];
    let mut itable_blk = vec![0u32; num_groups as usize];
    let mut data_start_blk = vec![0u32; num_groups as usize];

    for g in 0..num_groups as usize {
        let base = (g as u32) * BLOCKS_PER_GROUP;
        let meta_start = base + 1 + gdt_blocks as u32;
        bbm_blk[g] = meta_start;
        ibm_blk[g] = meta_start + 1;
        itable_blk[g] = meta_start + 2;
        data_start_blk[g] = meta_start + 2 + inode_table_blocks as u32;
    }

    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = File::create(out_path)
        .map_err(|e| format!("failed to create '{}': {}", out_path.display(), e))?;
    file.set_len(total_blocks as u64 * BLOCK_SIZE as u64)
        .map_err(|e| format!("failed to set image length: {}", e))?;

    let mut block_bitmaps = vec![vec![0u8; BLOCK_SIZE]; num_groups as usize];
    let mut inode_bitmaps = vec![vec![0u8; BLOCK_SIZE]; num_groups as usize];
    let mut free_blocks_group = vec![0u32; num_groups as usize];
    let mut free_inodes_group = vec![inodes_per_group; num_groups as usize];
    let mut used_dirs_group = vec![0u16; num_groups as usize];

    for g in 0..num_groups as usize {
        let g_start = (g as u32) * BLOCKS_PER_GROUP;
        let g_end = ((g as u32 + 1) * BLOCKS_PER_GROUP).min(total_blocks);
        let g_len = (g_end - g_start) as usize;
        let reserved = (data_start_blk[g] - g_start) as usize;
        for b in 0..reserved {
            set_bit(&mut block_bitmaps[g], b);
        }
        for b in g_len..BLOCKS_PER_GROUP as usize {
            set_bit(&mut block_bitmaps[g], b);
        }
        free_blocks_group[g] = (g_len - reserved) as u32;

        for i in inodes_per_group as usize..BLOCK_SIZE * 8 {
            set_bit(&mut inode_bitmaps[g], i);
        }
    }

    // Reserve inodes 1..=10 in group 0
    for i in 0..10 {
        set_bit(&mut inode_bitmaps[0], i);
        free_inodes_group[0] -= 1;
    }

    let mut cur_group = 0usize;
    let mut cur_blk = data_start_blk[0];
    let mut alloc_block = |file: &mut File, data: &[u8]| -> Result<u32, String> {
        while cur_group < num_groups as usize {
            let g_start = (cur_group as u32) * BLOCKS_PER_GROUP;
            let g_end = ((cur_group as u32 + 1) * BLOCKS_PER_GROUP).min(total_blocks);
            if cur_blk < g_end {
                let blk = cur_blk;
                cur_blk += 1;
                set_bit(&mut block_bitmaps[cur_group], (blk - g_start) as usize);
                free_blocks_group[cur_group] -= 1;
                file.seek(SeekFrom::Start(blk as u64 * BLOCK_SIZE as u64))
                    .map_err(|e| e.to_string())?;
                file.write_all(data).map_err(|e| e.to_string())?;
                return Ok(blk);
            }
            cur_group += 1;
            if cur_group < num_groups as usize {
                cur_blk = data_start_blk[cur_group];
            }
        }
        Err("ext2 image ran out of data blocks".to_string())
    };

    // 4. Write all inode payloads and inode table entries
    let ptrs_per_blk = BLOCK_SIZE / 4; // 1024
    let mut inode_table_bytes = vec![vec![0u8; inode_table_blocks * BLOCK_SIZE]; num_groups as usize];

    let ino_keys: Vec<u32> = specs.keys().copied().collect();
    for ino in ino_keys {
        let spec = specs.get(&ino).unwrap().clone();
        let g = ((ino - 1) / inodes_per_group) as usize;
        let idx = ((ino - 1) % inodes_per_group) as usize;
        if ino >= 11 {
            set_bit(&mut inode_bitmaps[g], idx);
            free_inodes_group[g] -= 1;
        }
        if spec.file_type == FT_DIR {
            used_dirs_group[g] += 1;
        }

        let mut raw_inode = [0u8; INODE_SIZE];
        raw_inode[0..2].copy_from_slice(&spec.mode.to_le_bytes());
        raw_inode[2..4].copy_from_slice(&spec.uid.to_le_bytes());
        raw_inode[24..26].copy_from_slice(&spec.gid.to_le_bytes());

        let (payload, size_bytes, links_count): (Vec<u8>, u32, u16) = if spec.file_type == FT_DIR {
            let dir_bytes = encode_dir_blocks(spec.ino, spec.parent_ino, &spec.children);
            let subdirs = spec.children.iter().filter(|(_, _, ft)| *ft == FT_DIR).count() as u16;
            (dir_bytes.clone(), dir_bytes.len() as u32, 2 + subdirs)
        } else if let Some(ref target) = spec.symlink {
            let tb = target.as_bytes();
            if tb.len() < 60 {
                raw_inode[4..8].copy_from_slice(&(tb.len() as u32).to_le_bytes());
                raw_inode[26..28].copy_from_slice(&1u16.to_le_bytes());
                raw_inode[40..40 + tb.len()].copy_from_slice(tb);
                let off = idx * INODE_SIZE;
                inode_table_bytes[g][off..off + INODE_SIZE].copy_from_slice(&raw_inode);
                continue;
            }
            (tb.to_vec(), tb.len() as u32, 1)
        } else {
            (spec.data.clone(), spec.data.len() as u32, 1)
        };

        let mut data_blks = Vec::new();
        for chunk in payload.chunks(BLOCK_SIZE) {
            let mut buf = [0u8; BLOCK_SIZE];
            buf[..chunk.len()].copy_from_slice(chunk);
            data_blks.push(alloc_block(&mut file, &buf)?);
        }

        let mut i_block = [0u32; 15];
        let mut total_512_sectors = (data_blks.len() * (BLOCK_SIZE / 512)) as u32;

        for (i, &b) in data_blks.iter().take(12).enumerate() {
            i_block[i] = b;
        }
        if data_blks.len() > 12 {
            let single_slice = &data_blks[12..data_blks.len().min(12 + ptrs_per_blk)];
            let mut ind_buf = [0u8; BLOCK_SIZE];
            for (i, &b) in single_slice.iter().enumerate() {
                ind_buf[i * 4..i * 4 + 4].copy_from_slice(&b.to_le_bytes());
            }
            i_block[12] = alloc_block(&mut file, &ind_buf)?;
            total_512_sectors += (BLOCK_SIZE / 512) as u32;
        }
        if data_blks.len() > 12 + ptrs_per_blk {
            let rem = &data_blks[12 + ptrs_per_blk..];
            let mut dind_buf = [0u8; BLOCK_SIZE];
            for (c_idx, chunk) in rem.chunks(ptrs_per_blk).enumerate() {
                let mut sind_buf = [0u8; BLOCK_SIZE];
                for (i, &b) in chunk.iter().enumerate() {
                    sind_buf[i * 4..i * 4 + 4].copy_from_slice(&b.to_le_bytes());
                }
                let sind_blk = alloc_block(&mut file, &sind_buf)?;
                total_512_sectors += (BLOCK_SIZE / 512) as u32;
                dind_buf[c_idx * 4..c_idx * 4 + 4].copy_from_slice(&sind_blk.to_le_bytes());
            }
            i_block[13] = alloc_block(&mut file, &dind_buf)?;
            total_512_sectors += (BLOCK_SIZE / 512) as u32;
        }

        raw_inode[4..8].copy_from_slice(&size_bytes.to_le_bytes());
        raw_inode[26..28].copy_from_slice(&links_count.to_le_bytes());
        raw_inode[28..32].copy_from_slice(&total_512_sectors.to_le_bytes());
        for (i, &ptr) in i_block.iter().enumerate() {
            raw_inode[40 + i * 4..44 + i * 4].copy_from_slice(&ptr.to_le_bytes());
        }

        let off = idx * INODE_SIZE;
        inode_table_bytes[g][off..off + INODE_SIZE].copy_from_slice(&raw_inode);
    }

    // 5. Write Superblock (at offset 1024 in block 0), GDT (block 1..), bitmaps, and inode tables
    let total_free_blocks: u32 = free_blocks_group.iter().sum();
    let total_free_inodes: u32 = free_inodes_group.iter().sum();

    let mut block0 = vec![0u8; BLOCK_SIZE];
    let sb = &mut block0[1024..2048];
    sb[0..4].copy_from_slice(&total_inodes.to_le_bytes());
    sb[4..8].copy_from_slice(&total_blocks.to_le_bytes());
    sb[8..12].copy_from_slice(&0u32.to_le_bytes());
    sb[12..16].copy_from_slice(&total_free_blocks.to_le_bytes());
    sb[16..20].copy_from_slice(&total_free_inodes.to_le_bytes());
    sb[20..24].copy_from_slice(&0u32.to_le_bytes()); // s_first_data_block = 0 for 4K blocks
    sb[24..28].copy_from_slice(&2u32.to_le_bytes()); // s_log_block_size = 2 (4096)
    sb[28..32].copy_from_slice(&2u32.to_le_bytes()); // s_log_frag_size = 2
    sb[32..36].copy_from_slice(&BLOCKS_PER_GROUP.to_le_bytes());
    sb[36..40].copy_from_slice(&BLOCKS_PER_GROUP.to_le_bytes());
    sb[40..44].copy_from_slice(&inodes_per_group.to_le_bytes());
    sb[52..54].copy_from_slice(&0u16.to_le_bytes());
    sb[54..56].copy_from_slice(&20u16.to_le_bytes());
    sb[56..58].copy_from_slice(&EXT2_SUPER_MAGIC.to_le_bytes());
    sb[58..60].copy_from_slice(&1u16.to_le_bytes()); // EXT2_VALID_FS
    sb[60..62].copy_from_slice(&1u16.to_le_bytes()); // EXT2_ERRORS_CONTINUE
    sb[76..80].copy_from_slice(&1u32.to_le_bytes()); // EXT2_DYNAMIC_REV
    sb[84..88].copy_from_slice(&11u32.to_le_bytes()); // s_first_ino
    sb[88..90].copy_from_slice(&(INODE_SIZE as u16).to_le_bytes());
    sb[96..100].copy_from_slice(&0x0002u32.to_le_bytes()); // EXT2_FEATURE_INCOMPAT_FILETYPE
    sb[104..120].copy_from_slice(b"KUYASE-EXT2-UUID");
    sb[120..126].copy_from_slice(b"system");

    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    file.write_all(&block0).map_err(|e| e.to_string())?;

    let mut gdt = vec![0u8; gdt_blocks * BLOCK_SIZE];
    for g in 0..num_groups as usize {
        let gd = &mut gdt[g * 32..g * 32 + 32];
        gd[0..4].copy_from_slice(&bbm_blk[g].to_le_bytes());
        gd[4..8].copy_from_slice(&ibm_blk[g].to_le_bytes());
        gd[8..12].copy_from_slice(&itable_blk[g].to_le_bytes());
        gd[12..14].copy_from_slice(&(free_blocks_group[g] as u16).to_le_bytes());
        gd[14..16].copy_from_slice(&(free_inodes_group[g] as u16).to_le_bytes());
        gd[16..18].copy_from_slice(&used_dirs_group[g].to_le_bytes());
    }
    file.seek(SeekFrom::Start(BLOCK_SIZE as u64))
        .map_err(|e| e.to_string())?;
    file.write_all(&gdt).map_err(|e| e.to_string())?;

    for g in 0..num_groups as usize {
        file.seek(SeekFrom::Start(bbm_blk[g] as u64 * BLOCK_SIZE as u64))
            .map_err(|e| e.to_string())?;
        file.write_all(&block_bitmaps[g]).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(ibm_blk[g] as u64 * BLOCK_SIZE as u64))
            .map_err(|e| e.to_string())?;
        file.write_all(&inode_bitmaps[g]).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(itable_blk[g] as u64 * BLOCK_SIZE as u64))
            .map_err(|e| e.to_string())?;
        file.write_all(&inode_table_bytes[g])
            .map_err(|e| e.to_string())?;
    }

    file.flush().map_err(|e| e.to_string())
}

pub fn read_ext2_image(path: &Path) -> Result<RootfsTree, String> {
    let mut file =
        File::open(path).map_err(|e| format!("failed to open '{}': {}", path.display(), e))?;
    let mut sb = [0u8; 1024];
    file.seek(SeekFrom::Start(1024))
        .and_then(|_| file.read_exact(&mut sb))
        .map_err(|e| format!("failed to read ext superblock: {}", e))?;

    let magic = u16::from_le_bytes([sb[56], sb[57]]);
    if magic != EXT2_SUPER_MAGIC {
        return Err(format!("invalid ext2/ext4 superblock magic: 0x{:04x}", magic));
    }

    let log_block_size = u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]);
    let blk_size = 1024usize << log_block_size;
    let inodes_per_group = u32::from_le_bytes([sb[40], sb[41], sb[42], sb[43]]);
    let rev = u32::from_le_bytes([sb[76], sb[77], sb[78], sb[79]]);
    let inode_size = if rev >= 1 {
        u16::from_le_bytes([sb[88], sb[89]]) as usize
    } else {
        128
    };
    let feature_incompat = u32::from_le_bytes([sb[96], sb[97], sb[98], sb[99]]);
    let desc_size = if (feature_incompat & EXT4_FEATURE_INCOMPAT_64BIT) != 0 {
        let s_desc_size = u16::from_le_bytes([sb[254], sb[255]]) as usize;
        if s_desc_size >= 32 {
            s_desc_size
        } else {
            32
        }
    } else {
        32
    };
    let gdt_offset = if blk_size == 1024 { 2048u64 } else { blk_size as u64 };

    let read_inode =
        |file: &mut File, ino: u32| -> Result<(u16, u16, u16, u32, u32, [u8; 60]), String> {
            let g = (ino - 1) / inodes_per_group;
            let idx = (ino - 1) % inodes_per_group;
            let mut gd = vec![0u8; desc_size];
            file.seek(SeekFrom::Start(gdt_offset + g as u64 * desc_size as u64))
                .and_then(|_| file.read_exact(&mut gd))
                .map_err(|e| e.to_string())?;
            let itable_lo = u32::from_le_bytes([gd[8], gd[9], gd[10], gd[11]]) as u64;
            let itable_hi = if desc_size >= 64 {
                u32::from_le_bytes([gd[40], gd[41], gd[42], gd[43]]) as u64
            } else {
                0
            };
            let itable = itable_lo | (itable_hi << 32);
            let mut raw = vec![0u8; inode_size];
            file.seek(SeekFrom::Start(
                itable * blk_size as u64 + idx as u64 * inode_size as u64,
            ))
            .and_then(|_| file.read_exact(&mut raw))
            .map_err(|e| e.to_string())?;
            let mode = u16::from_le_bytes([raw[0], raw[1]]);
            let uid = u16::from_le_bytes([raw[2], raw[3]]);
            let size = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
            let gid = u16::from_le_bytes([raw[24], raw[25]]);
            let flags = u32::from_le_bytes([raw[32], raw[33], raw[34], raw[35]]);
            let mut i_block = [0u8; 60];
            i_block.copy_from_slice(&raw[40..100]);
            Ok((mode, uid, gid, size, flags, i_block))
        };

    fn collect_extents(
        file: &mut File,
        node_bytes: &[u8],
        blk_size: usize,
        out_extents: &mut Vec<(u32, u16, u64)>,
    ) -> Result<(), String> {
        if node_bytes.len() < 12 {
            return Err("extent node too short".to_string());
        }
        let eh_magic = u16::from_le_bytes([node_bytes[0], node_bytes[1]]);
        if eh_magic != EXT4_EXTENT_MAGIC {
            return Err(format!("invalid ext4 extent magic: 0x{:04x}", eh_magic));
        }
        let eh_entries = u16::from_le_bytes([node_bytes[2], node_bytes[3]]) as usize;
        let eh_depth = u16::from_le_bytes([node_bytes[6], node_bytes[7]]);
        for idx in 0..eh_entries {
            let off = 12 + idx * 12;
            if off + 12 > node_bytes.len() {
                break;
            }
            let entry = &node_bytes[off..off + 12];
            if eh_depth == 0 {
                let ee_block = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                let ee_len = u16::from_le_bytes([entry[4], entry[5]]);
                let ee_start_hi = u16::from_le_bytes([entry[6], entry[7]]) as u64;
                let ee_start_lo =
                    u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as u64;
                let start_phys = ee_start_lo | (ee_start_hi << 32);
                out_extents.push((ee_block, ee_len, start_phys));
            } else {
                let ei_leaf_lo =
                    u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as u64;
                let ei_leaf_hi = u16::from_le_bytes([entry[8], entry[9]]) as u64;
                let child_blk = ei_leaf_lo | (ei_leaf_hi << 32);
                let mut child_buf = vec![0u8; blk_size];
                file.seek(SeekFrom::Start(child_blk * blk_size as u64))
                    .and_then(|_| file.read_exact(&mut child_buf))
                    .map_err(|e| e.to_string())?;
                collect_extents(file, &child_buf, blk_size, out_extents)?;
            }
        }
        Ok(())
    }

    let read_inode_data = |file: &mut File,
                           size: u32,
                           flags: u32,
                           i_block: &[u8; 60]|
     -> Result<Vec<u8>, String> {
        if (flags & EXT4_EXTENTS_FL) != 0
            && u16::from_le_bytes([i_block[0], i_block[1]]) == EXT4_EXTENT_MAGIC
        {
            let mut extents = Vec::new();
            collect_extents(file, i_block, blk_size, &mut extents)?;
            let mut out = vec![0u8; size as usize];
            let mut buf = vec![0u8; blk_size];
            for (ee_block, ee_len, start_phys) in extents {
                let uninit = ee_len > 32768;
                let len_blks = if uninit {
                    (ee_len - 32768) as usize
                } else {
                    ee_len as usize
                };
                if uninit {
                    continue;
                }
                for b_idx in 0..len_blks {
                    let logical_blk = (ee_block as usize) + b_idx;
                    let byte_off = logical_blk * blk_size;
                    if byte_off >= size as usize {
                        break;
                    }
                    file.seek(SeekFrom::Start((start_phys + b_idx as u64) * blk_size as u64))
                        .and_then(|_| file.read_exact(&mut buf))
                        .map_err(|e| e.to_string())?;
                    let take = (size as usize - byte_off).min(blk_size);
                    out[byte_off..byte_off + take].copy_from_slice(&buf[..take]);
                }
            }
            return Ok(out);
        }

        let mut ptrs = [0u32; 15];
        for i in 0..15 {
            ptrs[i] = u32::from_le_bytes([
                i_block[i * 4],
                i_block[i * 4 + 1],
                i_block[i * 4 + 2],
                i_block[i * 4 + 3],
            ]);
        }
        let needed_blks = (size as usize + blk_size - 1) / blk_size;
        let mut blk_nums = Vec::with_capacity(needed_blks);
        for &b in ptrs.iter().take(12) {
            if blk_nums.len() < needed_blks && b != 0 {
                blk_nums.push(b);
            }
        }
        let ptrs_per_blk = blk_size / 4;
        if blk_nums.len() < needed_blks && ptrs[12] != 0 {
            let mut ind = vec![0u8; blk_size];
            file.seek(SeekFrom::Start(ptrs[12] as u64 * blk_size as u64))
                .and_then(|_| file.read_exact(&mut ind))
                .map_err(|e| e.to_string())?;
            for i in 0..ptrs_per_blk {
                if blk_nums.len() >= needed_blks {
                    break;
                }
                let b = u32::from_le_bytes([ind[i * 4], ind[i * 4 + 1], ind[i * 4 + 2], ind[i * 4 + 3]]);
                if b != 0 {
                    blk_nums.push(b);
                }
            }
        }
        if blk_nums.len() < needed_blks && ptrs[13] != 0 {
            let mut dind = vec![0u8; blk_size];
            file.seek(SeekFrom::Start(ptrs[13] as u64 * blk_size as u64))
                .and_then(|_| file.read_exact(&mut dind))
                .map_err(|e| e.to_string())?;
            for c in 0..ptrs_per_blk {
                if blk_nums.len() >= needed_blks {
                    break;
                }
                let sind_blk = u32::from_le_bytes([
                    dind[c * 4],
                    dind[c * 4 + 1],
                    dind[c * 4 + 2],
                    dind[c * 4 + 3],
                ]);
                if sind_blk == 0 {
                    continue;
                }
                let mut sind = vec![0u8; blk_size];
                file.seek(SeekFrom::Start(sind_blk as u64 * blk_size as u64))
                    .and_then(|_| file.read_exact(&mut sind))
                    .map_err(|e| e.to_string())?;
                for i in 0..ptrs_per_blk {
                    if blk_nums.len() >= needed_blks {
                        break;
                    }
                    let b = u32::from_le_bytes([
                        sind[i * 4],
                        sind[i * 4 + 1],
                        sind[i * 4 + 2],
                        sind[i * 4 + 3],
                    ]);
                    if b != 0 {
                        blk_nums.push(b);
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(size as usize);
        let mut buf = vec![0u8; blk_size];
        for b in blk_nums {
            file.seek(SeekFrom::Start(b as u64 * blk_size as u64))
                .and_then(|_| file.read_exact(&mut buf))
                .map_err(|e| e.to_string())?;
            let take = (size as usize - out.len()).min(blk_size);
            out.extend_from_slice(&buf[..take]);
        }
        Ok(out)
    };

    let mut tree = RootfsTree::default();
    let mut stack = vec![(2u32, String::new())];
    while let Some((dir_ino, prefix)) = stack.pop() {
        let (_, _, _, size, flags, i_block) = read_inode(&mut file, dir_ino)?;
        let dir_bytes = read_inode_data(&mut file, size, flags, &i_block)?;
        let mut off = 0usize;
        while off + 8 <= dir_bytes.len() {
            let e_ino = u32::from_le_bytes([
                dir_bytes[off],
                dir_bytes[off + 1],
                dir_bytes[off + 2],
                dir_bytes[off + 3],
            ]);
            let rec_len = u16::from_le_bytes([dir_bytes[off + 4], dir_bytes[off + 5]]) as usize;
            let name_len = dir_bytes[off + 6] as usize;
            if rec_len == 0 || off + rec_len > dir_bytes.len() {
                break;
            }
            if e_ino != 0 && name_len > 0 && off + 8 + name_len <= dir_bytes.len() {
                let name = String::from_utf8_lossy(&dir_bytes[off + 8..off + 8 + name_len]).to_string();
                if name != "." && name != ".." && name != "lost+found" {
                    let rel = if prefix.is_empty() {
                        name
                    } else {
                        format!("{}/{}", prefix, name)
                    };
                    let (imode, iuid, igid, isize, iflags, iblk) = read_inode(&mut file, e_ino)?;
                    let fmt = imode & 0xF000;
                    let perm = imode & 0o7777;
                    if fmt == S_IFDIR {
                        tree.insert_dir_with_owner(&rel, perm, iuid, igid);
                        stack.push((e_ino, rel));
                    } else if fmt == S_IFLNK {
                        let is_extent = (iflags & EXT4_EXTENTS_FL) != 0
                            && u16::from_le_bytes([iblk[0], iblk[1]]) == EXT4_EXTENT_MAGIC;
                        let target = if (isize as usize) < 60 && !is_extent {
                            String::from_utf8_lossy(&iblk[..isize as usize]).to_string()
                        } else {
                            let bytes = read_inode_data(&mut file, isize, iflags, &iblk)?;
                            String::from_utf8_lossy(&bytes).to_string()
                        };
                        tree.insert_symlink_with_owner(&rel, target, iuid, igid);
                    } else if fmt == S_IFREG {
                        let bytes = read_inode_data(&mut file, isize, iflags, &iblk)?;
                        tree.insert_file_with_owner(&rel, bytes, perm, iuid, igid);
                    }
                }
            }
            off += rec_len;
        }
    }
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_ext4_extents_and_64bit_group_descriptors_and_uid_gid() {
        let tmp_img = std::env::temp_dir().join(format!(
            "kuyase-ext4-extents-test-{}.img",
            std::process::id()
        ));
        let _ = fs::remove_file(&tmp_img);

        let mut src_tree = RootfsTree::default();
        src_tree.insert_dir_with_owner("etc", 0o750, 100, 200);
        src_tree.insert_file_with_owner(
            "etc/owned.conf",
            b"owner-test".to_vec(),
            0o640,
            1001,
            1002,
        );
        write_ext2_image(&src_tree, &tmp_img, 16 * 1024 * 1024).unwrap();

        // Now patch the image in-place to use:
        // 1) 64-byte group descriptor (INCOMPAT_64BIT = 0x0080, s_desc_size = 64)
        // 2) EXT4_EXTENTS_FL (0x00080000) + eh_magic 0xF30A on inode 12 ("etc/owned.conf"):
        //    use an internal index node (eh_depth = 1) pointing to an external leaf extent block!
        let mut raw = fs::read(&tmp_img).unwrap();

        // Enable INCOMPAT_64BIT (0x0080) and s_desc_size = 64 in superblock (at 1024)
        let incompat = u32::from_le_bytes([raw[1024 + 96], raw[1024 + 97], raw[1024 + 98], raw[1024 + 99]]);
        raw[1024 + 96..1024 + 100].copy_from_slice(&(incompat | EXT4_FEATURE_INCOMPAT_64BIT).to_le_bytes());
        raw[1024 + 254..1024 + 256].copy_from_slice(&64u16.to_le_bytes());

        // Read inode table block from GDT group 0 (block 1 = offset 4096)
        let itable_blk = u32::from_le_bytes([raw[4096 + 8], raw[4096 + 9], raw[4096 + 10], raw[4096 + 11]]) as usize;
        // Locate inode 12 ("etc/owned.conf"): index = 12 - 1 = 11
        let ino12_off = itable_blk * BLOCK_SIZE + 11 * INODE_SIZE;
        let orig_data_blk = u32::from_le_bytes([
            raw[ino12_off + 40],
            raw[ino12_off + 41],
            raw[ino12_off + 42],
            raw[ino12_off + 43],
        ]);

        // Use block 3000 as an external extent leaf node (eh_depth = 0, eh_entries = 1 -> orig_data_blk)
        let ext_leaf_blk = 3000usize;
        let leaf_off = ext_leaf_blk * BLOCK_SIZE;
        raw[leaf_off..leaf_off + 2].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
        raw[leaf_off + 2..leaf_off + 4].copy_from_slice(&1u16.to_le_bytes()); // eh_entries = 1
        raw[leaf_off + 4..leaf_off + 6].copy_from_slice(&340u16.to_le_bytes()); // eh_max
        raw[leaf_off + 6..leaf_off + 8].copy_from_slice(&0u16.to_le_bytes()); // eh_depth = 0 (leaf)
        // Leaf ext4_extent at leaf_off + 12
        raw[leaf_off + 12..leaf_off + 16].copy_from_slice(&0u32.to_le_bytes()); // ee_block = 0
        raw[leaf_off + 16..leaf_off + 18].copy_from_slice(&1u16.to_le_bytes()); // ee_len = 1
        raw[leaf_off + 18..leaf_off + 20].copy_from_slice(&0u16.to_le_bytes()); // ee_start_hi = 0
        raw[leaf_off + 20..leaf_off + 24].copy_from_slice(&orig_data_blk.to_le_bytes()); // ee_start_lo

        // Set EXT4_EXTENTS_FL on inode 12 and write eh_depth = 1 internal extent_idx in i_block[0..24]
        raw[ino12_off + 32..ino12_off + 36].copy_from_slice(&EXT4_EXTENTS_FL.to_le_bytes());
        raw[ino12_off + 40..ino12_off + 100].fill(0);
        raw[ino12_off + 40..ino12_off + 42].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
        raw[ino12_off + 42..ino12_off + 44].copy_from_slice(&1u16.to_le_bytes()); // eh_entries = 1
        raw[ino12_off + 44..ino12_off + 46].copy_from_slice(&4u16.to_le_bytes()); // eh_max = 4
        raw[ino12_off + 46..ino12_off + 48].copy_from_slice(&1u16.to_le_bytes()); // eh_depth = 1 (internal idx)
        // ext4_extent_idx at ino12_off + 52
        raw[ino12_off + 52..ino12_off + 56].copy_from_slice(&0u32.to_le_bytes()); // ei_block = 0
        raw[ino12_off + 56..ino12_off + 60].copy_from_slice(&(ext_leaf_blk as u32).to_le_bytes()); // ei_leaf_lo

        fs::write(&tmp_img, &raw).unwrap();

        let parsed = read_ext2_image(&tmp_img).unwrap();
        let etc_dir = parsed.get("etc").unwrap();
        assert_eq!(etc_dir.mode, 0o750);
        assert_eq!(etc_dir.uid, 100);
        assert_eq!(etc_dir.gid, 200);

        let owned = parsed.get("etc/owned.conf").unwrap();
        assert_eq!(owned.mode, 0o640);
        assert_eq!(owned.uid, 1001);
        assert_eq!(owned.gid, 1002);
        assert_eq!(parsed.get_file_bytes("etc/owned.conf").unwrap(), b"owner-test");

        let _ = fs::remove_file(&tmp_img);
    }
}
