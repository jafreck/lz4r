// lorem.rs — Lorem ipsum text generator
// Translated from lz4-1.10.0/programs/lorem.c
// Copyright (C) Yann Collet 2024 — GPL v2

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Word pool
// ---------------------------------------------------------------------------

static K_WORDS: &[&str] = &[
    "lorem",        "ipsum",      "dolor",       "sit",          "amet",
    "consectetur",  "adipiscing", "elit",        "sed",          "do",
    "eiusmod",      "tempor",     "incididunt",  "ut",           "labore",
    "et",           "dolore",     "magna",       "aliqua",       "dis",
    "lectus",       "vestibulum", "mattis",      "ullamcorper",  "velit",
    "commodo",      "a",          "lacus",       "arcu",         "magnis",
    "parturient",   "montes",     "nascetur",    "ridiculus",    "mus",
    "mauris",       "nulla",      "malesuada",   "pellentesque", "eget",
    "gravida",      "in",         "dictum",      "non",          "erat",
    "nam",          "voluptat",   "maecenas",    "blandit",      "aliquam",
    "etiam",        "enim",       "lobortis",    "scelerisque",  "fermentum",
    "dui",          "faucibus",   "ornare",      "at",           "elementum",
    "eu",           "facilisis",  "odio",        "morbi",        "quis",
    "eros",         "donec",      "ac",          "orci",         "purus",
    "turpis",       "cursus",     "leo",         "vel",          "porta",
    "consequat",    "interdum",   "varius",      "vulputate",    "aliquet",
    "pharetra",     "nunc",       "auctor",      "urna",         "id",
    "metus",        "viverra",    "nibh",        "cras",         "mi",
    "unde",         "omnis",      "iste",        "natus",        "error",
    "perspiciatis", "voluptatem", "accusantium", "doloremque",   "laudantium",
    "totam",        "rem",        "aperiam",     "eaque",        "ipsa",
    "quae",         "ab",         "illo",        "inventore",    "veritatis",
    "quasi",        "architecto", "beatae",      "vitae",        "dicta",
    "sunt",         "explicabo",  "nemo",        "ipsam",        "quia",
    "voluptas",     "aspernatur", "aut",         "odit",         "fugit",
    "consequuntur", "magni",      "dolores",     "eos",          "qui",
    "ratione",      "sequi",      "nesciunt",    "neque",        "porro",
    "quisquam",     "est",        "dolorem",     "adipisci",     "numquam",
    "eius",         "modi",       "tempora",     "incidunt",     "magnam",
    "quaerat",      "ad",         "minima",      "veniam",       "nostrum",
    "ullam",        "corporis",   "suscipit",    "laboriosam",   "nisi",
    "aliquid",      "ex",         "ea",          "commodi",      "consequatur",
    "autem",        "eum",        "iure",        "voluptate",    "esse",
    "quam",         "nihil",      "molestiae",   "illum",        "fugiat",
    "quo",          "pariatur",   "vero",        "accusamus",    "iusto",
    "dignissimos",  "ducimus",    "blanditiis",  "praesentium",  "voluptatum",
    "deleniti",     "atque",      "corrupti",    "quos",         "quas",
    "molestias",    "excepturi",  "sint",        "occaecati",    "cupiditate",
    "provident",    "similique",  "culpa",       "officia",      "deserunt",
    "mollitia",     "animi",      "laborum",     "dolorum",      "fuga",
    "harum",        "quidem",     "rerum",       "facilis",      "expedita",
    "distinctio",   "libero",     "tempore",     "cum",          "soluta",
    "nobis",        "eligendi",   "optio",       "cumque",       "impedit",
    "minus",        "quod",       "maxime",      "placeat",      "facere",
    "possimus",     "assumenda",  "repellendus", "temporibus",   "quibusdam",
    "officiis",     "debitis",    "saepe",       "eveniet",      "voluptates",
    "repudiandae",  "recusandae", "itaque",      "earum",        "hic",
    "tenetur",      "sapiente",   "delectus",    "reiciendis",   "cillum",
    "maiores",      "alias",      "perferendis", "doloribus",    "asperiores",
    "repellat",     "minim",      "nostrud",     "exercitation", "ullamco",
    "laboris",      "aliquip",    "duis",        "aute",         "irure",
];

/// Weight by word-length index (clamped to last entry for len >= 5).
/// Mirrors: static const int kWeights[] = { 0, 8, 6, 4, 3, 2 };
static K_WEIGHTS: &[i32] = &[0, 8, 6, 4, 3, 2];

// ---------------------------------------------------------------------------
// Word pool — lazy-initialised once
// ---------------------------------------------------------------------------

struct WordPool {
    word_lens: Vec<usize>,
    /// Distribution table: each entry is an index into K_WORDS.
    distrib: Vec<usize>,
    distrib_count: usize,
}

static WORD_POOL: OnceLock<WordPool> = OnceLock::new();

fn get_pool() -> &'static WordPool {
    WORD_POOL.get_or_init(|| {
        let nb_weights = K_WEIGHTS.len();
        let word_lens: Vec<usize> = K_WORDS.iter().map(|w| w.len()).collect();

        // countFreqs equivalent
        let distrib_count: usize = word_lens
            .iter()
            .map(|&len| K_WEIGHTS[len.min(nb_weights - 1)].max(0) as usize)
            .sum();

        // init_word_distrib equivalent
        let mut distrib = Vec::with_capacity(distrib_count);
        for (w, &len) in word_lens.iter().enumerate() {
            let lmax = K_WEIGHTS[len.min(nb_weights - 1)].max(0) as usize;
            for _ in 0..lmax {
                distrib.push(w);
            }
        }

        WordPool { word_lens, distrib, distrib_count }
    })
}

// ---------------------------------------------------------------------------
// Per-call generation context (replaces C file-scope globals)
// ---------------------------------------------------------------------------

struct GenCtx<'a> {
    buf: &'a mut [u8],
    nb_chars: usize,
    max_chars: usize,
    rand_root: u32,
}

impl<'a> GenCtx<'a> {
    /// LOREM_rand: custom 32-bit PRNG (XOR-rotation hash).
    #[inline]
    fn lorem_rand(&mut self, range: u32) -> u32 {
        const PRIME1: u32 = 2_654_435_761;
        const PRIME2: u32 = 2_246_822_519;
        let mut r = self.rand_root;
        r = r.wrapping_mul(PRIME1);
        r ^= PRIME2;
        r = r.rotate_left(13);
        self.rand_root = r;
        ((r as u64 * range as u64) >> 32) as u32
    }

    /// about(target) = LOREM_rand(target) + LOREM_rand(target) + 1
    #[inline]
    fn about(&mut self, target: u32) -> u32 {
        self.lorem_rand(target) + self.lorem_rand(target) + 1
    }

    /// writeLastCharacters: fill remaining buffer with `. <spaces>\n`.
    fn write_last_characters(&mut self) {
        debug_assert!(self.max_chars >= self.nb_chars);
        let last_chars = self.max_chars - self.nb_chars;
        if last_chars == 0 {
            return;
        }
        self.buf[self.nb_chars] = b'.';
        self.nb_chars += 1;
        if last_chars > 2 {
            let fill_end = self.nb_chars + (last_chars - 2);
            self.buf[self.nb_chars..fill_end].fill(b' ');
        }
        if last_chars > 1 {
            self.buf[self.max_chars - 1] = b'\n';
        }
        self.nb_chars = self.max_chars;
    }

    /// generateLastWord: write a word then flush remaining characters.
    fn generate_last_word(&mut self, word: &[u8], up_case: bool) {
        let word_len = word.len();
        if self.nb_chars + word_len + 2 > self.max_chars {
            self.write_last_characters();
            return;
        }
        let dst = self.nb_chars;
        self.buf[dst..dst + word_len].copy_from_slice(word);
        if up_case {
            // 'A' - 'a' = 32; uppercase the first byte
            self.buf[dst] = self.buf[dst].wrapping_sub(32);
        }
        self.nb_chars += word_len;
        self.write_last_characters();
    }

    /// generateWord: write a word + separator; falls back to generateLastWord
    /// when near the end of the buffer.
    ///
    /// The C version copies 16 bytes unconditionally (perf trick) but only
    /// advances nb_chars by wordLen; copying exactly wordLen bytes is
    /// behaviourally equivalent for the visible output.
    fn generate_word(&mut self, word: &[u8], sep: &[u8], up_case: bool) {
        let word_len = word.len();
        let sep_len = sep.len();
        // wlen = MAX(16, wordLen + 2) — minimum headroom needed
        let wlen = 16usize.max(word_len + 2);
        if self.nb_chars + wlen > self.max_chars {
            self.generate_last_word(word, up_case);
            return;
        }
        let dst = self.nb_chars;
        // Copy word (C copies 16 bytes; we copy exactly wordLen — same result)
        self.buf[dst..dst + word_len].copy_from_slice(word);
        if up_case {
            self.buf[dst] = self.buf[dst].wrapping_sub(32);
        }
        self.nb_chars += word_len;
        // C always memcpy(sep, 2) then advances by sepLen; we copy exactly sepLen
        let sdst = self.nb_chars;
        self.buf[sdst..sdst + sep_len].copy_from_slice(sep);
        self.nb_chars += sep_len;
    }

    /// generateSentence
    fn generate_sentence(&mut self, nb_words: u32, pool: &WordPool) {
        let comma_pos = self.about(9);
        let comma2 = comma_pos + self.about(7);
        let qmark = self.lorem_rand(11) == 7;
        let end_sep: &[u8] = if qmark { b"? " } else { b". " };

        for i in 0..nb_words {
            let word_id = pool.distrib[self.lorem_rand(pool.distrib_count as u32) as usize];
            let word = K_WORDS[word_id].as_bytes();
            let sep: &[u8] = if i == nb_words - 1 {
                end_sep
            } else if i == comma_pos || i == comma2 {
                b", "
            } else {
                b" "
            };
            self.generate_word(word, sep, i == 0);
        }
    }

    /// generateParagraph
    fn generate_paragraph(&mut self, nb_sentences: u32, pool: &WordPool) {
        for _ in 0..nb_sentences {
            let words_per_sentence = self.about(11);
            self.generate_sentence(words_per_sentence, pool);
        }
        if self.nb_chars < self.max_chars {
            self.buf[self.nb_chars] = b'\n';
            self.nb_chars += 1;
        }
        if self.nb_chars < self.max_chars {
            self.buf[self.nb_chars] = b'\n';
            self.nb_chars += 1;
        }
    }

    /// generateFirstSentence: always starts with "Lorem ipsum dolor sit amet, ..."
    fn generate_first_sentence(&mut self, pool: &WordPool) {
        for i in 0..18usize {
            let sep: &[u8] = if i == 4 || i == 7 { b", " } else { b" " };
            let word = K_WORDS[i].as_bytes();
            let word_len = pool.word_lens[i];
            self.generate_word(&word[..word_len], sep, i == 0);
        }
        // word 18 with ". " separator
        let word = K_WORDS[18].as_bytes();
        let word_len = pool.word_lens[18];
        self.generate_word(&word[..word_len], b". ", false);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate at most `buf.len()` bytes of lorem ipsum text.
///
/// Equivalent to `LOREM_genBlock` in C.
///
/// - `first`: if true, prepend the canonical "Lorem ipsum…" opening sentence.
/// - `fill`:  if true, fill the entire buffer; otherwise generate one paragraph.
///
/// Returns the number of bytes actually written.
pub fn gen_block(buf: &mut [u8], seed: u32, first: bool, fill: bool) -> usize {
    let pool = get_pool();
    let max_chars = buf.len();
    let mut ctx = GenCtx {
        buf,
        nb_chars: 0,
        max_chars,
        rand_root: seed,
    };

    if first {
        ctx.generate_first_sentence(pool);
    }

    while ctx.nb_chars < ctx.max_chars {
        let sentences_per_paragraph = ctx.about(7);
        ctx.generate_paragraph(sentences_per_paragraph, pool);
        if !fill {
            break; // only one paragraph in non-fill mode
        }
    }

    ctx.nb_chars
}

/// Fill a `Vec<u8>` of exactly `size` bytes with lorem ipsum text.
///
/// Equivalent to `LOREM_genBuffer` in C.
pub fn gen_buffer(size: usize, seed: u32) -> Vec<u8> {
    let mut buf = vec![0u8; size];
    gen_block(&mut buf, seed, true, true);
    buf
}
