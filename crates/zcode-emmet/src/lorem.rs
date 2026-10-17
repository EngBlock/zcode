//! Lorem ipsum (`lorem`, `lorem10`, `lipsum5-10`): Emmet's generator with a
//! fixed seed, so the same abbreviation always gives the same text.

use crate::error::Error;

/// The most words one `lorem` may ask for.
const MAX_WORDS: usize = 10_000;
/// The most words one expansion may generate in all.
const MAX_TOTAL: usize = 100_000;
/// Emmet's default length.
const DEFAULT_WORDS: usize = 30;

/// The first sentence of the first (or only) lorem.
const COMMON: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipisicing",
    "elit",
];

const WORDS: &[&str] = &[
    "exercitationem",
    "perferendis",
    "perspiciatis",
    "laborum",
    "eveniet",
    "sunt",
    "iure",
    "nam",
    "nobis",
    "eum",
    "cum",
    "officiis",
    "excepturi",
    "odio",
    "consectetur",
    "quasi",
    "aut",
    "quisquam",
    "vel",
    "eligendi",
    "itaque",
    "non",
    "odit",
    "tempore",
    "quaerat",
    "dignissimos",
    "facilis",
    "neque",
    "nihil",
    "expedita",
    "vitae",
    "vero",
    "ipsum",
    "nisi",
    "animi",
    "cumque",
    "pariatur",
    "velit",
    "modi",
    "natus",
    "iusto",
    "eaque",
    "sequi",
    "illo",
    "sed",
    "ex",
    "et",
    "voluptatibus",
    "tempora",
    "veritatis",
    "ratione",
    "assumenda",
    "incidunt",
    "nostrum",
    "placeat",
    "aliquid",
    "fuga",
    "provident",
    "praesentium",
    "rem",
    "necessitatibus",
    "suscipit",
    "adipisci",
    "quidem",
    "possimus",
    "voluptas",
    "debitis",
    "sint",
    "accusantium",
    "unde",
    "sapiente",
    "voluptate",
    "qui",
    "aspernatur",
    "laudantium",
    "soluta",
    "amet",
    "quo",
    "aliquam",
    "saepe",
    "culpa",
    "libero",
    "ipsa",
    "dicta",
    "reiciendis",
    "nesciunt",
    "doloribus",
    "autem",
    "impedit",
    "minima",
    "maiores",
    "repudiandae",
    "ipsam",
    "obcaecati",
    "ullam",
    "enim",
    "totam",
    "delectus",
    "ducimus",
    "quis",
    "voluptates",
    "dolores",
    "molestiae",
    "harum",
    "dolorem",
    "quia",
    "voluptatem",
    "molestias",
    "magni",
    "distinctio",
    "omnis",
    "illum",
    "dolorum",
    "voluptatum",
    "ea",
    "quas",
    "quam",
    "corporis",
    "quae",
    "blanditiis",
    "atque",
    "deserunt",
    "laboriosam",
    "earum",
    "consequuntur",
    "hic",
    "cupiditate",
    "quibusdam",
    "accusamus",
    "ut",
    "rerum",
    "error",
    "minus",
    "eius",
    "ab",
    "ad",
    "nemo",
    "fugit",
    "officia",
    "at",
    "in",
    "id",
    "quos",
    "reprehenderit",
    "numquam",
    "iste",
    "fugiat",
    "sit",
    "inventore",
    "beatae",
    "repellendus",
    "magnam",
    "recusandae",
    "quod",
    "explicabo",
    "doloremque",
    "aperiam",
    "consequatur",
    "asperiores",
    "commodi",
    "optio",
    "dolor",
    "labore",
    "temporibus",
    "repellat",
    "veniam",
    "architecto",
    "est",
    "esse",
    "mollitia",
    "nulla",
    "a",
    "similique",
    "eos",
    "alias",
    "dolore",
    "tenetur",
    "deleniti",
    "porro",
    "facere",
    "maxime",
    "corrupti",
];

/// The word count of a lorem name (`lorem`, `lorem10`, `lipsum`, `lorem5-10`
/// takes the lower bound), or `None` when `name` isn't one.
pub(crate) fn word_count(name: &str) -> Option<usize> {
    let rest = name
        .strip_prefix("lorem")
        .or_else(|| name.strip_prefix("lipsum"))?;
    let (count, upper) = match rest.split_once('-') {
        Some((count, upper)) => (count, Some(upper)),
        None => (rest, None),
    };
    if !count.bytes().all(|b| b.is_ascii_digit())
        || upper.is_some_and(|u| !u.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if count.is_empty() {
        return Some(DEFAULT_WORDS);
    }
    Some(count.parse::<usize>().unwrap_or(usize::MAX).max(1))
}

/// A deterministic lorem generator for one expansion.
pub(crate) struct Lorem {
    state: u64,
    total: usize,
}

impl Lorem {
    pub(crate) fn new() -> Self {
        Self {
            state: 0x5EED_1E55_10E3_0001,
            total: 0,
        }
    }

    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `low..=high`.
    fn range(&mut self, low: usize, high: usize) -> usize {
        low + (self.next() % (high - low + 1) as u64) as usize
    }

    /// A paragraph of `count` words in sentences; the first begins "Lorem
    /// ipsum dolor sit amet…" when `start_with_common`.
    pub(crate) fn paragraph(
        &mut self,
        count: usize,
        start_with_common: bool,
    ) -> Result<String, Error> {
        self.total = self.total.saturating_add(count);
        if count > MAX_WORDS || self.total > MAX_TOTAL {
            return Err(Error::TooLarge);
        }
        let mut sentences = Vec::new();
        let mut written = 0;
        if start_with_common {
            let words: Vec<String> = COMMON.iter().take(count).map(|w| (*w).to_owned()).collect();
            written += words.len();
            sentences.push(sentence(words, '.'));
        }
        while written < count {
            let length = self.range(3, 12).min(count - written);
            let mut words: Vec<String> = (0..length)
                .map(|_| WORDS[self.range(0, WORDS.len() - 1)].to_owned())
                .collect();
            self.insert_commas(&mut words);
            let end = ['?', '!', '.', '.', '.'][self.range(0, 4)];
            written += words.len();
            sentences.push(sentence(words, end));
        }
        Ok(sentences.join(" "))
    }

    /// Emmet's commas: none in short sentences, up to two in longer ones.
    fn insert_commas(&mut self, words: &mut [String]) {
        let commas = match words.len() {
            0..=3 => 0,
            4..=6 => self.range(0, 1),
            _ => self.range(0, 2),
        };
        for _ in 0..commas {
            let at = self.range(0, words.len() - 2);
            if !words[at].ends_with(',') {
                words[at].push(',');
            }
        }
    }
}

fn sentence(mut words: Vec<String>, end: char) -> String {
    if let Some(first) = words.first_mut()
        && let Some(initial) = first.chars().next()
    {
        let capitalized: String = initial
            .to_uppercase()
            .chain(first.chars().skip(1))
            .collect();
        *first = capitalized;
    }
    let mut sentence = words.join(" ");
    sentence.push(end);
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(word_count("lorem"), Some(30));
        assert_eq!(word_count("lipsum"), Some(30));
        assert_eq!(word_count("lorem4"), Some(4));
        assert_eq!(word_count("lorem0"), Some(1));
        assert_eq!(word_count("lorem5-10"), Some(5));
        assert_eq!(word_count("loremx"), None);
        assert_eq!(word_count("lor"), None);
    }

    #[test]
    fn deterministic_and_counted() {
        let text = Lorem::new().paragraph(30, true).unwrap();
        assert!(text.starts_with("Lorem ipsum dolor sit amet consectetur adipisicing elit. "));
        assert_eq!(text.split_whitespace().count(), 30);
        assert_eq!(text, Lorem::new().paragraph(30, true).unwrap());
        assert_eq!(
            Lorem::new().paragraph(4, true).unwrap(),
            "Lorem ipsum dolor sit."
        );
        let other = Lorem::new().paragraph(12, false).unwrap();
        assert_eq!(other.split_whitespace().count(), 12);
        assert!(!other.starts_with("Lorem"));
        assert!(Lorem::new().paragraph(MAX_WORDS + 1, false).is_err());
    }
}
