use crate::completion::privacy::bare_word;
use crate::completion::types::{CandidateKind, CompletionContext};
use flate2::bufread::ZlibDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;

const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;
const EMBEDDED_CATALOG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/catalog.json.zlib"));

fn decompress_catalog(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    // Read one byte beyond the limit to distinguish a valid boundary-sized
    // catalog from oversized output without decompressing the entire stream.
    let mut decoder = ZlibDecoder::new(data).take((MAX_CATALOG_BYTES + 1) as u64);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|_| "Invalid compressed completion catalog")?;
    if decoded.len() > MAX_CATALOG_BYTES {
        return Err("Completion catalog exceeds size limit");
    }
    if !decoder.into_inner().into_inner().is_empty() {
        return Err("Invalid compressed completion catalog");
    }
    Ok(decoded)
}

#[derive(Deserialize)]
pub struct Catalog {
    version: u32,
    pub roots: Vec<CommandNode>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}
#[derive(Deserialize)]
pub struct CommandNode {
    pub token: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub hidden: bool,
    pub flags: Vec<CommandFlag>,
    pub persistent: Vec<CommandFlag>,
    pub children: Vec<CommandNode>,
}
#[derive(Deserialize)]
pub struct CommandFlag {
    pub long: String,
    pub short: String,
    pub description: String,
    pub takes_value: bool,
    pub optional: bool,
    pub repeatable: bool,
    pub required: bool,
    pub values: Vec<String>,
}
impl CommandFlag {
    pub fn token(&self) -> String {
        if self.long.is_empty() {
            format!("-{}", self.short)
        } else {
            format!("--{}", self.long)
        }
    }
    fn matches(&self, value: &str) -> bool {
        (!self.long.is_empty() && value.strip_prefix("--") == Some(self.long.as_str()))
            || (!self.short.is_empty() && value.strip_prefix('-') == Some(self.short.as_str()))
    }
}

impl Catalog {
    pub fn embedded() -> Result<Self, &'static str> {
        Self::decode(&decompress_catalog(EMBEDDED_CATALOG)?)
    }
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() > MAX_CATALOG_BYTES {
            return Err("Completion catalog exceeds size limit");
        }
        let mut catalog: Self =
            serde_json::from_slice(data).map_err(|_| "Invalid completion catalog")?;
        if catalog.version != 1 || catalog.roots.len() > 20000 {
            return Err("Unsupported completion catalog");
        }
        fn valid(node: &CommandNode, depth: usize) -> bool {
            depth <= 32
                && bare_word(&node.token)
                && node.aliases.iter().all(|a| bare_word(a))
                && node.flags.iter().chain(&node.persistent).all(|f| {
                    (!f.long.is_empty() || !f.short.is_empty())
                        && f.values.iter().all(|v| bare_word(v))
                })
                && node.children.iter().all(|n| valid(n, depth + 1))
        }
        for (i, root) in catalog.roots.iter().enumerate() {
            if !valid(root, 0) || catalog.index.insert(root.token.clone(), i).is_some() {
                return Err("Invalid completion catalog tree");
            }
            for alias in &root.aliases {
                catalog.index.entry(alias.clone()).or_insert(i);
            }
        }
        Ok(catalog)
    }

    /// Resolves only the active tree. Unknown options stop static inference.
    pub fn candidates(
        &self,
        context: &mut CompletionContext,
    ) -> Vec<(String, String, CandidateKind)> {
        let mut result = Vec::new();
        if context.preceding.is_empty() {
            for root in self
                .roots
                .iter()
                .filter(|n| !n.hidden && !context.dialect.native_options(&n.token))
            {
                result.push((
                    root.token.clone(),
                    root.description.clone(),
                    CandidateKind::Command,
                ));
                result.extend(
                    root.aliases
                        .iter()
                        .map(|a| (a.clone(), root.description.clone(), CandidateKind::Command)),
                );
            }
            return result;
        }
        if context.dialect.native_options(&context.preceding[0]) {
            return result;
        }
        let root = if context.dialect.case_insensitive_commands() {
            context.preceding[0].to_ascii_lowercase()
        } else {
            context.preceding[0].clone()
        };
        let Some(&index) = self.index.get(&root) else {
            return result;
        };
        let mut node = &self.roots[index];
        context.command_path.push(node.token.clone());
        let mut inherited = Vec::new();
        let mut expected: Option<&CommandFlag> = None;
        let mut stopped = false;
        let mut positional = false;
        let mut used = Vec::new();
        for word in &context.preceding[1..] {
            if expected.take().is_some() {
                continue;
            }
            if !stopped && word == "--" {
                stopped = true;
                continue;
            }
            let flags: Vec<_> = inherited
                .iter()
                .copied()
                .chain(node.persistent.iter())
                .chain(node.flags.iter())
                .collect();
            if !stopped && word.starts_with('-') {
                let (name, value) = word
                    .split_once('=')
                    .map_or((word.as_str(), None), |(n, v)| (n, Some(v)));
                if let Some(flag) = flags.iter().copied().find(|f| f.matches(name)) {
                    used.push(flag.token());
                    if value.is_none() && flag.takes_value && !flag.optional {
                        expected = Some(flag);
                    }
                } else if !word.starts_with("--") && word.len() > 2 {
                    let chars = word[1..].char_indices();
                    for (offset, ch) in chars {
                        let short = format!("-{ch}");
                        let Some(flag) = flags.iter().copied().find(|f| f.matches(&short)) else {
                            return Vec::new();
                        };
                        used.push(flag.token());
                        if flag.takes_value {
                            if offset + ch.len_utf8() == word.len() - 1 && !flag.optional {
                                expected = Some(flag);
                            }
                            break;
                        }
                    }
                } else {
                    return Vec::new();
                }
                continue;
            }
            if !positional
                && !stopped
                && let Some(child) = node
                    .children
                    .iter()
                    .find(|n| n.token == *word || n.aliases.contains(word))
            {
                inherited.extend(&node.persistent);
                node = child;
                context.command_path.push(node.token.clone());
                continue;
            }
            positional = true;
        }
        let flags: Vec<_> = inherited
            .iter()
            .copied()
            .chain(node.persistent.iter())
            .chain(node.flags.iter())
            .collect();
        if !stopped && let Some((name, _)) = context.prefix.split_once('=') {
            if let Some(flag) = flags
                .iter()
                .copied()
                .find(|f| f.matches(name) && f.takes_value)
            {
                let name = name.to_string();
                context.token_range.start += name.len() + 1;
                context.prefix = context.input[context.token_range.clone()].to_string();
                context.preceding.push(name);
                expected = Some(flag);
            } else {
                return Vec::new();
            }
        }
        if !stopped && let Some(name) = context.option_value.as_deref() {
            expected = flags
                .iter()
                .copied()
                .find(|f| f.matches(name) && f.takes_value);
        }
        if let Some(flag) = expected {
            context.option_value = Some(flag.token());
            return flag
                .values
                .iter()
                .map(|v| (v.clone(), flag.description.clone(), CandidateKind::Argument))
                .collect();
        }
        if !positional && !stopped && !context.prefix.starts_with('-') {
            for child in node.children.iter().filter(|n| !n.hidden) {
                result.push((
                    child.token.clone(),
                    child.description.clone(),
                    CandidateKind::Subcommand,
                ));
                result.extend(child.aliases.iter().map(|a| {
                    (
                        a.clone(),
                        child.description.clone(),
                        CandidateKind::Subcommand,
                    )
                }));
            }
        }
        if !stopped && (context.prefix.is_empty() || context.prefix.starts_with('-')) {
            for flag in flags {
                if !flag.repeatable && used.contains(&flag.token()) {
                    continue;
                }
                result.push((
                    flag.token(),
                    flag.description.clone(),
                    CandidateKind::Option,
                ));
                if !flag.short.is_empty() {
                    result.push((
                        format!("-{}", flag.short),
                        flag.description.clone(),
                        CandidateKind::Option,
                    ));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::completion::catalog::{
        Catalog, EMBEDDED_CATALOG, MAX_CATALOG_BYTES, decompress_catalog,
    };
    use crate::completion::parser::parse_context;
    use crate::completion::types::{CompletionScope, ShellDialect};
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    const ORIGINAL_CATALOG: &[u8] = include_bytes!("../../assets/completion/catalog.json");

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn embedded_compression_preserves_every_source_byte_and_reduces_resource_size() {
        assert_eq!(
            decompress_catalog(EMBEDDED_CATALOG).unwrap(),
            ORIGINAL_CATALOG
        );
        assert!(EMBEDDED_CATALOG.len() * 100 < ORIGINAL_CATALOG.len() * 15);
    }

    #[test]
    fn compressed_catalog_preserves_command_subcommand_alias_and_option_candidates() {
        let embedded = Catalog::embedded().unwrap();
        let original = Catalog::decode(ORIGINAL_CATALOG).unwrap();
        for input in [
            "",
            "docker ",
            "docker run --",
            "git log --",
            "git stage --",
            "bazelisk --",
            "kubectl get ",
        ] {
            let mut expected_context = parse_context(
                input,
                input.len(),
                ShellDialect::Bash,
                CompletionScope::legacy(),
            )
            .unwrap();
            let mut actual_context = expected_context.clone();
            let expected = original.candidates(&mut expected_context);
            assert!(!expected.is_empty(), "catalog fixture must cover {input:?}");
            assert_eq!(
                embedded.candidates(&mut actual_context),
                expected,
                "{input:?}"
            );
            assert_eq!(actual_context.command_path, expected_context.command_path);
            assert_eq!(actual_context.option_value, expected_context.option_value);
            assert_eq!(actual_context.token_range, expected_context.token_range);
            assert_eq!(actual_context.prefix, expected_context.prefix);
        }
    }

    #[test]
    fn decompression_rejects_truncated_headers_payloads_and_checksums() {
        for end in [
            0,
            1,
            2,
            EMBEDDED_CATALOG.len() / 2,
            EMBEDDED_CATALOG.len() - 1,
        ] {
            assert_eq!(
                decompress_catalog(&EMBEDDED_CATALOG[..end]),
                Err("Invalid compressed completion catalog"),
                "truncation at {end}"
            );
        }
    }

    #[test]
    fn decompression_rejects_corrupt_headers_and_checksums() {
        for index in [0, EMBEDDED_CATALOG.len() - 1] {
            let mut corrupt = EMBEDDED_CATALOG.to_vec();
            corrupt[index] ^= 0xff;
            assert_eq!(
                decompress_catalog(&corrupt),
                Err("Invalid compressed completion catalog")
            );
        }
    }

    #[test]
    fn decompression_rejects_trailing_data_and_concatenated_streams() {
        for trailing in [b"trailing".as_slice(), EMBEDDED_CATALOG] {
            let mut data = EMBEDDED_CATALOG.to_vec();
            data.extend_from_slice(trailing);
            assert_eq!(
                decompress_catalog(&data),
                Err("Invalid compressed completion catalog")
            );
        }
    }

    #[test]
    fn compressed_catalog_accepts_exact_size_limit_and_rejects_one_byte_more() {
        let mut source = br#"{"version":1,"roots":[]}"#.to_vec();
        source.resize(MAX_CATALOG_BYTES, b' ');
        let decoded = decompress_catalog(&compress(&source)).unwrap();
        assert_eq!(decoded.len(), MAX_CATALOG_BYTES);
        assert!(Catalog::decode(&decoded).unwrap().roots.is_empty());
        source.push(b' ');
        assert_eq!(
            decompress_catalog(&compress(&source)),
            Err("Completion catalog exceeds size limit")
        );
        assert!(matches!(
            Catalog::decode(&source),
            Err("Completion catalog exceeds size limit")
        ));
    }

    fn fixture() -> Catalog {
        let flag = |long, short, takes_value, values: Vec<&str>| {
            serde_json::json!({
            "long":long,"short":short,"takes_value":takes_value,"optional":false,"repeatable":false,"required":false,"description":"fixture","values":values})
        };
        let data = serde_json::json!({"version":1,"roots":[{"token":"tool","aliases":["alias"],"description":"fixture","hidden":false,
            "flags":[],"persistent":[flag("global","g",false,vec![])],"children":[{"token":"sub","aliases":["s"],"description":"fixture","hidden":false,"persistent":[],
            "flags":[flag("verbose","v",false,vec![]),flag("mode","m",true,vec!["json","yaml"])],"children":[{"token":"action","aliases":[],"description":"fixture","hidden":false,"flags":[],"persistent":[],"children":[]}]}]}]});
        Catalog::decode(&serde_json::to_vec(&data).unwrap()).unwrap()
    }
    #[test]
    fn follows_aliases_inherited_flags_inline_values_and_known_short_clusters() {
        let catalog = fixture();
        for (input, expected, value) in [
            ("alias s ", "--global", false),
            ("tool sub --mode j", "json", true),
            ("tool sub --mode=js", "json", true),
            ("tool sub -vm j", "json", true),
            ("tool sub ", "action", false),
        ] {
            let mut context = parse_context(
                input,
                input.len(),
                ShellDialect::Bash,
                CompletionScope::legacy(),
            )
            .unwrap();
            let candidates = catalog.candidates(&mut context);
            assert!(candidates.iter().any(|(text, _, _)| text == expected));
            assert_eq!(context.option_value.is_some(), value);
            assert_eq!(&input[context.token_range.clone()], context.prefix);
        }
        for input in [
            "tool sub -- --mo",
            "tool sub -- --mode=j",
            "tool sub --unknown x ",
            "tool sub -z ",
        ] {
            let mut context = parse_context(
                input,
                input.len(),
                ShellDialect::Bash,
                CompletionScope::legacy(),
            )
            .unwrap();
            assert!(catalog.candidates(&mut context).is_empty());
        }
        let input = "tool sub --mode json ";
        let mut context = parse_context(
            input,
            input.len(),
            ShellDialect::Bash,
            CompletionScope::legacy(),
        )
        .unwrap();
        assert!(
            !catalog
                .candidates(&mut context)
                .iter()
                .any(|(text, _, _)| text == "--mode")
        );
    }
}
