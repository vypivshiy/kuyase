//! # Модуль легковесного формантного синтеза речи (`tts.rs`)
//!
//! Реализует процедурную (0 КБ внешних словарей и весов, `< 0.2% CPU`) реконструкцию
//! русской речи голосового ассистента («Алисы») по входной UTF-8 строке в поток
//! стерео-PCM (`48000 Гц`, `S16_LE`).
//!
//! ## Что необходимо для дешёвой реконструкции голоса Алисы
//!
//! Чтобы синтезированный без нейросетей сигнал воспринимался на слух и на спектрограмме
//! как живая русская речь ассистента (и длился ровно столько же, сколько реальный TTS),
//! достаточно пяти низкозатратных компонентов:
//!
//! 1. **Нормализация числительных (`Number Expansion`)**:
//!    Цифры (`21`, `18`, `100`) в тексте занимают 1–3 байта, но произносятся как длинные
//!    многосложные слова (*«двадцать один»*). Развёртка чисел в слова до фонетического
//!    разбора обеспечивает точную слоговую длительность фразы.
//! 2. **Слияние клитик и ритмико-слоговая структура (`Proclitic Merging & Stress`)**:
//!    В русском языке предлоги и союзы (`в`, `без`, `на`, `не`, `по`, `за`, `из`, `под`)
//!    произносятся слитно со следующим словом как единое фонетическое слово с одним
//!    ударением (*«в Москве» -> «вмаскве́»*, *«без осадков» -> «бизаса́ткаф»*). Ударный слог
//!    почти вдвое длиннее и громче безударных (`152..175 мс` против `82 мс`).
//! 3. **Русская орфоэпия, редукция и палатализация (`Orthoepy & Palatalization`)**:
//!    * **Аканье и иканье**: безударное `о` переходит в `[а]`, предударные `е/я` — в `[и]`.
//!    * **Позиционное оглушение**: звонкие шумные (`б, в, г, д, ж, з`) на конце слова и
//!      перед глухими переходят в парные глухие (`п, ф, к, т, ш, с`).
//!    * **Палатализация (смягчение)**: перед гласными переднего ряда (`е, ё, и, ю, я`) и `ь`
//!      вторая форманта `F2` согласного сдвигается вверх к локусу `2180..2380 Гц`.
//! 4. **Русские интонационные конструкции Брызгуновой (`ИК-1`, `ИК-2`, `ИК-3`)**:
//!    * Вдоль каждой синтагмы действует плавный наклон базовой частоты (`declination`).
//!    * Внутри каждого слога тон `F0` строится по 3-точечной купольной кривой (`f0_s -> f0_m -> f0_e`).
//!    * `ИК-1` (точка `.`): падение тона на последнем слове вниз до `0.72 * F0`.
//!    * `ИК-2` (восклицание `!`): высокий акцентный купол `1.44 * F0` с резким спадом.
//!    * `ИК-3` (вопрос `?`): крутой подъём тона до `1.58 * F0` на ударном слоге последнего слова.
//!    * Незавершённая синтагма (запятая `,` / тире `—`): подъём `1.28 * F0` перед паузой `210 мс`.
//! 5. **Акустическая модель «источник — фильтр» (`Rosenberg + Klatt Formant Vocoder`)**:
//!    * **Источник голосового возбуждения**: импульс Розенберга (`Rosenberg glottal pulse` —
//!      фаза открытия связок `42%`, фаза смыкания `16%`) с дифференциацией излучения на губах,
//!      вибрато `5.6 Гц`, микро-джиттером периода и придыхательным шумом (`aspiration`).
//!    * **Источник шумового возбуждения**: генератор `xorshift32` через полосовой резонатор
//!      для фрикативных (`с, ш, щ, ч, ф, х`) и взрывных (`к, п, т`) согласных.
//!    * **Голосовой тракт**: гибридный каскадно-параллельный банк из 4 биквадратных (IIR)
//!      резонаторов (`F1`, `F2`, `F3` + фиксированная форманта присутствия `F4 = 3450 Гц`)
//!      с непрерывной коартикуляцией (экспоненциальным сглаживанием частот между фонемами).

/// Двухполюсный цифровой резонатор (Bandpass IIR фильтр Клаута).
struct Resonator {
    sr: f32,
    y1: f32,
    y2: f32,
    a: f32,
    b: f32,
    c: f32,
}

impl Resonator {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            y1: 0.0,
            y2: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        }
    }

    fn set_params(&mut self, freq: f32, bw: f32) {
        let f = freq.clamp(90.0, self.sr * 0.44);
        let r = (-std::f32::consts::PI * bw / self.sr).exp();
        self.c = -(r * r);
        self.b = 2.0 * r * (2.0 * std::f32::consts::PI * f / self.sr).cos();
        self.a = 1.0 - self.b - self.c;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn is_vowel(ch: char) -> bool {
    matches!(
        ch,
        'а' | 'е' | 'ё' | 'и' | 'о' | 'у' | 'ы' | 'э' | 'ю' | 'я'
            | 'a' | 'e' | 'i' | 'o' | 'u' | 'y'
    )
}

fn is_softening(ch: char) -> bool {
    matches!(
        ch,
        'е' | 'ё' | 'и' | 'ю' | 'я' | 'ь' | 'ч' | 'щ' | 'й' | 'e' | 'i' | 'y'
    )
}

fn is_voiceless(ch: char) -> bool {
    matches!(
        ch,
        'п' | 'ф' | 'к' | 'т' | 'ш' | 'с' | 'х' | 'ц' | 'ч' | 'щ' | 'p' | 'f' | 'k' | 't' | 's' | 'h'
    )
}

fn is_proclitic(w: &str) -> bool {
    matches!(
        w,
        "в" | "во" | "на" | "за" | "по" | "из" | "от" | "до" | "без"
            | "под" | "над" | "к" | "ко" | "у" | "с" | "со" | "о" | "об"
            | "не" | "ни" | "и" | "а" | "но"
    )
}

fn number_to_russian_words(n: u32) -> String {
    let units = [
        "ноль", "один", "два", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять",
    ];
    let teens = [
        "десять",
        "одиннадцать",
        "двенадцать",
        "тринадцать",
        "четырнадцать",
        "пятнадцать",
        "шестнадцать",
        "семнадцать",
        "восемнадцать",
        "девятнадцать",
    ];
    let tens = [
        "",
        "",
        "двадцать",
        "тридцать",
        "сорок",
        "пятьдесят",
        "шестьдесят",
        "семьдесят",
        "восемьдесят",
        "девяносто",
    ];
    let hundreds = [
        "",
        "сто",
        "двести",
        "триста",
        "четыреста",
        "пятьсот",
        "шестьсот",
        "семьсот",
        "восемьсот",
        "девятьсот",
    ];
    if n < 10 {
        return units[n as usize].to_string();
    }
    if n < 20 {
        return teens[(n - 10) as usize].to_string();
    }
    if n < 100 {
        let t = (n / 10) as usize;
        let u = (n % 10) as usize;
        return if u == 0 {
            tens[t].to_string()
        } else {
            format!("{} {}", tens[t], units[u])
        };
    }
    if n < 1000 {
        let h = (n / 100) as usize;
        let rem = n % 100;
        return if rem == 0 {
            hundreds[h].to_string()
        } else {
            format!("{} {}", hundreds[h], number_to_russian_words(rem))
        };
    }
    n.to_string()
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| units[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn expand_numbers(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            if !digits.is_empty() {
                if let Ok(val) = digits.parse::<u32>() {
                    out.push(' ');
                    out.push_str(&number_to_russian_words(val));
                    out.push(' ');
                }
                digits.clear();
            }
            out.push(ch);
        }
    }
    if !digits.is_empty() {
        if let Ok(val) = digits.parse::<u32>() {
            out.push(' ');
            out.push_str(&number_to_russian_words(val));
        }
    }
    out
}

fn russian_orthoepy(word: &str) -> Vec<char> {
    let w = word
        .replace("сегодня", "сиводня")
        .replace("что", "што")
        .replace("ться", "ца")
        .replace("тся", "ца");
    let mut chars: Vec<char> = w.chars().collect();
    let n = chars.len();
    for i in (0..n).rev() {
        let devoice = match chars[i] {
            'б' => Some('п'),
            'в' => Some('ф'),
            'г' => Some('к'),
            'д' => Some('т'),
            'ж' => Some('ш'),
            'з' => Some('с'),
            _ => None,
        };
        if let Some(rep) = devoice {
            let mut nxt = None;
            for &c2 in &chars[(i + 1)..n] {
                if c2 != 'ь' && c2 != 'ъ' {
                    nxt = Some(c2);
                    break;
                }
            }
            if nxt.is_none() || nxt.map(is_voiceless).unwrap_or(false) {
                chars[i] = rep;
            }
        }
    }
    chars
}

fn pick_stressed_vowel_index(word: &[char]) -> isize {
    let v_indices: Vec<usize> = word
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| if is_vowel(c) { Some(i) } else { None })
        .collect();
    if v_indices.is_empty() {
        return -1;
    }
    for &i in &v_indices {
        if word[i] == 'ё' {
            return i as isize;
        }
    }
    if v_indices.len() == 1 {
        return v_indices[0] as isize;
    }
    let s: String = word.iter().collect();
    if v_indices.len() == 2 {
        if s.ends_with("ет")
            || s.ends_with("ве")
            || s.ends_with("ить")
            || s.ends_with("ать")
            || s.ends_with("ой")
            || s.ends_with("ей")
            || s.ends_with("ом")
            || s.ends_with("ин")
            || s.ends_with("им")
        {
            return v_indices[1] as isize;
        }
        return v_indices[0] as isize;
    }
    if s.starts_with("муз") || s.starts_with("двад") || s.starts_with("вык") {
        return v_indices[0] as isize;
    }
    v_indices[1] as isize
}

struct PhonemeEvent {
    dur_ms: u32,
    f1: f32,
    f2: f32,
    f3: f32,
    v_amp: f32,
    n_amp: f32,
    n_freq: f32,
    f0_s: f32,
    f0_m: f32,
    f0_e: f32,
    trill: bool,
}

/// Синтезирует стерео-буфер PCM (`48000 Гц`, `S16_LE`, 2 канала) и возвращает
/// кортеж `(stereo_samples, duration_ms)`.
pub fn synthesize_speech_pcm(text: &str) -> (Vec<i16>, u32) {
    const SR: f32 = 48000.0;
    const BASE_F0: f32 = 225.0;

    let expanded = expand_numbers(&text.to_lowercase());
    let mut clauses: Vec<(String, char)> = Vec::new();
    let mut cur_clause = String::new();
    for ch in expanded.chars() {
        if matches!(ch, ',' | '.' | ';' | ':' | '!' | '?' | '—' | '-') {
            if !cur_clause.trim().is_empty() {
                clauses.push((cur_clause.trim().to_string(), ch));
                cur_clause.clear();
            } else if let Some(last) = clauses.last_mut() {
                if ch == '?' || ch == '!' {
                    last.1 = ch;
                }
            }
        } else {
            cur_clause.push(ch);
        }
    }
    if !cur_clause.trim().is_empty() {
        clauses.push((cur_clause.trim().to_string(), '.'));
    }

    let mut events: Vec<PhonemeEvent> = Vec::new();

    for (clause_body, punct) in clauses {
        let raw_words: Vec<&str> = clause_body
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .collect();
        if raw_words.is_empty() {
            continue;
        }
        let mut merged_words: Vec<(String, String)> = Vec::new();
        let mut carry = String::new();
        for w in raw_words {
            if is_proclitic(w) {
                carry.push_str(w);
            } else if !carry.is_empty() {
                merged_words.push((carry.clone(), w.to_string()));
                carry.clear();
            } else {
                merged_words.push((String::new(), w.to_string()));
            }
        }
        if !carry.is_empty() {
            if let Some(last) = merged_words.last_mut() {
                last.1.push_str(&carry);
            } else {
                merged_words.push((String::new(), carry));
            }
        }

        // Короткий вдох перед началом синтагмы
        events.push(PhonemeEvent {
            dur_ms: 26,
            f1: 500.0,
            f2: 1450.0,
            f3: 2500.0,
            v_amp: 0.0,
            n_amp: 0.06,
            n_freq: 1600.0,
            f0_s: BASE_F0,
            f0_m: BASE_F0,
            f0_e: BASE_F0,
            trill: false,
        });

        let num_words = merged_words.len();
        for (w_idx, (proclitic, content_word)) in merged_words.into_iter().enumerate() {
            let is_last_word = w_idx + 1 == num_words;
            let decl = 1.12 - 0.22 * (w_idx as f32 / (num_words.saturating_sub(1).max(1) as f32));

            let content_chars: Vec<char> = content_word.chars().collect();
            let proc_len = proclitic.chars().count() as isize;
            let stress_in_content = pick_stressed_vowel_index(&content_chars);
            let stress_pos = if stress_in_content >= 0 {
                proc_len + stress_in_content
            } else {
                -1
            };
            let full_raw = format!("{}{}", proclitic, content_word);
            let word = russian_orthoepy(&full_raw);

            for (c_idx, &ch) in word.iter().enumerate() {
                if ch == 'ь' || ch == 'ъ' {
                    continue;
                }
                let nxt_ch = word.get(c_idx + 1).copied().unwrap_or(' ');
                let is_soft = is_softening(nxt_ch) || matches!(ch, 'ч' | 'щ' | 'й');
                let is_stressed = (c_idx as isize) == stress_pos;

                let eff_ch = if !is_stressed {
                    if ch == 'о' {
                        'а'
                    } else if (ch == 'е' || ch == 'я') && (c_idx as isize) < stress_pos {
                        'и'
                    } else {
                        ch
                    }
                } else {
                    ch
                };

                let mut f0_s = BASE_F0 * decl * 0.96;
                let mut f0_m = BASE_F0 * decl * 1.00;
                let mut f0_e = BASE_F0 * decl * 0.95;

                if is_stressed {
                    if is_last_word && punct == '?' {
                        f0_s = BASE_F0 * 0.92;
                        f0_m = BASE_F0 * 1.32;
                        f0_e = BASE_F0 * 1.58;
                    } else if is_last_word && punct == '!' {
                        f0_s = BASE_F0 * 1.12;
                        f0_m = BASE_F0 * 1.44;
                        f0_e = BASE_F0 * 1.05;
                    } else if is_last_word && matches!(punct, ',' | ';' | ':' | '—' | '-') {
                        f0_s = BASE_F0 * 0.92;
                        f0_m = BASE_F0 * 1.14;
                        f0_e = BASE_F0 * 1.28;
                    } else if is_last_word && punct == '.' {
                        f0_s = BASE_F0 * 1.06;
                        f0_m = BASE_F0 * 0.92;
                        f0_e = BASE_F0 * 0.72;
                    } else {
                        f0_s = BASE_F0 * decl * 1.00;
                        f0_m = BASE_F0 * decl * 1.24;
                        f0_e = BASE_F0 * decl * 1.06;
                    }
                } else if is_last_word && stress_pos >= 0 && (c_idx as isize) > stress_pos {
                    if punct == '?' {
                        f0_s = BASE_F0 * 1.50;
                        f0_m = BASE_F0 * 1.30;
                        f0_e = BASE_F0 * 1.15;
                    } else if punct == '.' {
                        f0_s = BASE_F0 * 0.74;
                        f0_m = BASE_F0 * 0.69;
                        f0_e = BASE_F0 * 0.66;
                    } else if matches!(punct, ',' | ';' | ':' | '—' | '-') {
                        f0_s = BASE_F0 * 1.24;
                        f0_m = BASE_F0 * 1.28;
                        f0_e = BASE_F0 * 1.30;
                    }
                }

                let vowel_formants = match eff_ch {
                    'а' | 'a' => Some((760.0, 1280.0, 2550.0)),
                    'я' => Some((680.0, 1620.0, 2680.0)),
                    'о' | 'o' => Some((460.0, 860.0, 2400.0)),
                    'ё' => Some((440.0, 1140.0, 2480.0)),
                    'у' | 'u' => Some((300.0, 650.0, 2300.0)),
                    'ю' => Some((290.0, 940.0, 2380.0)),
                    'э' => Some((520.0, 1880.0, 2620.0)),
                    'е' | 'e' => Some((430.0, 2120.0, 2780.0)),
                    'и' | 'i' => Some((270.0, 2380.0, 3050.0)),
                    'ы' | 'y' => Some((335.0, 1540.0, 2460.0)),
                    _ => None,
                };

                if let Some((f1, f2, f3)) = vowel_formants {
                    let mut dur = if is_stressed { 152 } else { 82 };
                    if is_last_word && is_stressed {
                        dur = 175;
                    }
                    let amp = if is_stressed { 1.20 } else { 0.76 };
                    if matches!(ch, 'я' | 'е' | 'ё' | 'ю') {
                        events.push(PhonemeEvent {
                            dur_ms: (dur * 28) / 100,
                            f1: 280.0,
                            f2: 2280.0,
                            f3: 2980.0,
                            v_amp: amp * 0.88,
                            n_amp: 0.02,
                            n_freq: 2300.0,
                            f0_s,
                            f0_m: (f0_s + f0_m) * 0.5,
                            f0_e: f0_m,
                            trill: false,
                        });
                        events.push(PhonemeEvent {
                            dur_ms: (dur * 72) / 100,
                            f1,
                            f2,
                            f3,
                            v_amp: amp,
                            n_amp: 0.02,
                            n_freq: 2000.0,
                            f0_s: f0_m,
                            f0_m,
                            f0_e,
                            trill: false,
                        });
                    } else {
                        events.push(PhonemeEvent {
                            dur_ms: dur,
                            f1,
                            f2,
                            f3,
                            v_amp: amp,
                            n_amp: 0.02,
                            n_freq: 2000.0,
                            f0_s,
                            f0_m,
                            f0_e,
                            trill: false,
                        });
                    }
                    continue;
                }

                let sonorant: Option<(f32, f32, f32, bool)> = match eff_ch {
                    'м' | 'm' => Some((250.0, 800.0, 2200.0, false)),
                    'н' | 'n' => Some((260.0, 1680.0, 2500.0, false)),
                    'л' | 'l' => Some((330.0, 1000.0, 2550.0, false)),
                    'р' | 'r' => Some((390.0, 1440.0, 2250.0, true)),
                    'й' | 'j' => Some((255.0, 2420.0, 3100.0, false)),
                    'в' | 'v' | 'w' => Some((290.0, 840.0, 2300.0, false)),
                    'б' | 'b' => Some((270.0, 780.0, 2200.0, false)),
                    'д' | 'd' => Some((280.0, 1740.0, 2550.0, false)),
                    'г' | 'g' => Some((280.0, 1980.0, 2350.0, false)),
                    _ => None,
                };

                if let Some((f1, mut f2, mut f3, trill)) = sonorant {
                    if is_soft {
                        f2 = (f2 + 480.0).min(2380.0);
                        f3 = (f3 + 260.0).min(3020.0);
                    }
                    events.push(PhonemeEvent {
                        dur_ms: 46,
                        f1,
                        f2,
                        f3,
                        v_amp: 0.52,
                        n_amp: 0.02,
                        n_freq: 1900.0,
                        f0_s: f0_s * 0.95,
                        f0_m: f0_s * 0.96,
                        f0_e: f0_e * 0.95,
                        trill,
                    });
                } else if matches!(eff_ch, 'с' | 'ц' | 'з' | 's' | 'z' | 'c') {
                    let v_amp = if eff_ch == 'з' || eff_ch == 'z' { 0.38 } else { 0.0 };
                    let nf = if is_soft { 4900.0 } else { 4350.0 };
                    events.push(PhonemeEvent {
                        dur_ms: 50,
                        f1: 330.0,
                        f2: if is_soft { 1850.0 } else { 1700.0 },
                        f3: 2750.0,
                        v_amp,
                        n_amp: 0.34,
                        n_freq: nf,
                        f0_s,
                        f0_m,
                        f0_e,
                        trill: false,
                    });
                } else if matches!(eff_ch, 'ш' | 'щ' | 'ч' | 'ж') {
                    let v_amp = if eff_ch == 'ж' { 0.38 } else { 0.0 };
                    let nf = if eff_ch == 'щ' || eff_ch == 'ч' { 3150.0 } else { 2450.0 };
                    events.push(PhonemeEvent {
                        dur_ms: 54,
                        f1: 360.0,
                        f2: if eff_ch == 'щ' || eff_ch == 'ч' { 1950.0 } else { 1550.0 },
                        f3: 2500.0,
                        v_amp,
                        n_amp: 0.36,
                        n_freq: nf,
                        f0_s,
                        f0_m,
                        f0_e,
                        trill: false,
                    });
                } else if matches!(eff_ch, 'ф' | 'х' | 'f' | 'h') {
                    events.push(PhonemeEvent {
                        dur_ms: 42,
                        f1: 390.0,
                        f2: 1250.0,
                        f3: 2300.0,
                        v_amp: 0.0,
                        n_amp: 0.26,
                        n_freq: 1750.0,
                        f0_s,
                        f0_m,
                        f0_e,
                        trill: false,
                    });
                } else if matches!(eff_ch, 'к' | 'п' | 'т' | 'k' | 'p' | 't' | 'q') {
                    let f2_loc = if is_soft {
                        2180.0
                    } else if eff_ch == 'п' || eff_ch == 'p' {
                        840.0
                    } else if eff_ch == 'т' || eff_ch == 't' {
                        1740.0
                    } else {
                        1450.0
                    };
                    let nf = if is_soft { 3450.0 } else { 2350.0 };
                    events.push(PhonemeEvent {
                        dur_ms: 24,
                        f1: 320.0,
                        f2: f2_loc,
                        f3: 2400.0,
                        v_amp: 0.0,
                        n_amp: 0.0,
                        n_freq: nf,
                        f0_s,
                        f0_m: f0_s,
                        f0_e: f0_s,
                        trill: false,
                    });
                    events.push(PhonemeEvent {
                        dur_ms: 20,
                        f1: 380.0,
                        f2: f2_loc,
                        f3: 2500.0,
                        v_amp: 0.14,
                        n_amp: 0.34,
                        n_freq: nf,
                        f0_s,
                        f0_m,
                        f0_e,
                        trill: false,
                    });
                }
            }

            if !is_last_word {
                let f0_gap = BASE_F0 * decl * 0.94;
                events.push(PhonemeEvent {
                    dur_ms: 36,
                    f1: 440.0,
                    f2: 1300.0,
                    f3: 2450.0,
                    v_amp: 0.0,
                    n_amp: 0.0,
                    n_freq: 1500.0,
                    f0_s: f0_gap,
                    f0_m: f0_gap,
                    f0_e: f0_gap,
                    trill: false,
                });
            }
        }

        let pause_ms = if matches!(punct, ',' | ';' | ':' | '—' | '-') {
            210
        } else {
            340
        };
        let f0_p = BASE_F0 * 0.82;
        events.push(PhonemeEvent {
            dur_ms: pause_ms,
            f1: 440.0,
            f2: 1300.0,
            f3: 2450.0,
            v_amp: 0.0,
            n_amp: 0.0,
            n_freq: 1500.0,
            f0_s: f0_p,
            f0_m: f0_p,
            f0_e: f0_p,
            trill: false,
        });
    }

    if events.is_empty() {
        events.push(PhonemeEvent {
            dur_ms: 400,
            f1: 500.0,
            f2: 1500.0,
            f3: 2500.0,
            v_amp: 0.9,
            n_amp: 0.0,
            n_freq: 2000.0,
            f0_s: BASE_F0,
            f0_m: BASE_F0 * 1.15,
            f0_e: BASE_F0 * 0.85,
            trill: false,
        });
    }

    let total_dur_ms: u32 = events.iter().map(|e| e.dur_ms).sum();
    let total_frames = ((SR as u64 * total_dur_ms as u64) / 1000) as usize;
    let mut mono = Vec::with_capacity(total_frames + 1024);

    let mut res1 = Resonator::new(SR);
    let mut res2 = Resonator::new(SR);
    let mut res3 = Resonator::new(SR);
    let mut res4 = Resonator::new(SR);
    res4.set_params(3450.0, 210.0);
    let mut res_noise = Resonator::new(SR);

    let mut cur_f1 = 500.0f32;
    let mut cur_f2 = 1300.0f32;
    let mut cur_f3 = 2450.0f32;
    let mut cur_f0 = BASE_F0;
    let mut cur_v = 0.0f32;
    let mut cur_n = 0.0f32;
    let mut phase = 0.0f32;
    let mut prev_glottal = 0.0f32;
    let mut rng = 0x9E37_79B9u32;

    for ev in &events {
        let samples = ((SR * (ev.dur_ms as f32)) / 1000.0).max(1.0) as usize;
        res_noise.set_params(ev.n_freq, 850.0);
        for i in 0..samples {
            let frac = (i as f32) / (samples as f32);
            let target_f0 = if frac < 0.45 {
                let u = frac / 0.45;
                ev.f0_s + (ev.f0_m - ev.f0_s) * (u * std::f32::consts::FRAC_PI_2).sin()
            } else {
                let u = (frac - 0.45) / 0.55;
                ev.f0_m + (ev.f0_e - ev.f0_m) * (1.0 - (u * std::f32::consts::FRAC_PI_2).cos())
            };

            let alpha = 0.006f32;
            cur_f1 += (ev.f1 - cur_f1) * alpha;
            cur_f2 += (ev.f2 - cur_f2) * alpha;
            cur_f3 += (ev.f3 - cur_f3) * alpha;
            cur_f0 += (target_f0 - cur_f0) * 0.010;

            let mut env_target = ev.v_amp;
            if frac < 0.14 {
                env_target *= ((frac / 0.14) * std::f32::consts::FRAC_PI_2).sin();
            } else if frac > 0.80 {
                env_target *= (((frac - 0.80) / 0.20) * std::f32::consts::FRAC_PI_2).cos();
            }
            cur_v += (env_target - cur_v) * 0.028;
            cur_n += (ev.n_amp - cur_n) * 0.032;

            if (i & 15) == 0 {
                res1.set_params(cur_f1, 68.0);
                res2.set_params(cur_f2, 100.0);
                res3.set_params(cur_f3, 145.0);
            }

            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let noise = ((rng & 0xFFFF) as f32) / 32768.0 - 1.0;

            let t_sec = (mono.len() as f32) / SR;
            let vibrato = 2.0 * (2.0 * std::f32::consts::PI * 5.6 * t_sec).sin();
            let f0 = cur_f0 + vibrato + noise * 0.6;
            phase = (phase + f0 / SR).fract();

            let (g, aspiration) = if phase < 0.42 {
                (
                    0.5 * (1.0 - (std::f32::consts::PI * phase / 0.42).cos()),
                    noise * 0.035,
                )
            } else if phase < 0.58 {
                (
                    (0.5 * std::f32::consts::PI * (phase - 0.42) / 0.16).cos(),
                    0.0,
                )
            } else {
                (0.0, 0.0)
            };

            let mut glottal = (g - prev_glottal) * 6.0
                + (2.0 * std::f32::consts::PI * phase).sin() * 0.35
                + aspiration;
            prev_glottal = g;

            if ev.trill {
                glottal *= 0.42 + 0.58 * (2.0 * std::f32::consts::PI * 27.5 * t_sec).sin();
            }

            let r1 = res1.process(glottal);
            let r2 = res2.process(r1 * 0.60 + glottal * 0.40);
            let r3 = res3.process(r2 * 0.50 + glottal * 0.50);
            let r4 = res4.process(glottal);
            let voiced_sig = (r1 * 0.50 + r2 * 0.52 + r3 * 0.30 + r4 * 0.10) * cur_v;
            let unvoiced_sig = res_noise.process(noise) * cur_n;
            mono.push(voiced_sig + unvoiced_sig);
        }
    }

    let peak = mono
        .iter()
        .fold(0.0f32, |acc, &x| acc.max(x.abs()))
        .max(0.001);
    let scale = 24000.0 / peak;
    let mut stereo_pcm = Vec::with_capacity(mono.len() * 2);
    for s in mono {
        let v = (s * scale).clamp(-32000.0, 32000.0) as i16;
        stereo_pcm.push(v);
        stereo_pcm.push(v);
    }
    (stereo_pcm, total_dur_ms)
}
