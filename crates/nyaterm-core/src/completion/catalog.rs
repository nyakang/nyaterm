use crate::completion::privacy::bare_word;
use crate::completion::types::{CandidateKind, CompletionContext};
use flate2::bufread::ZlibDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;

const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;
const EMBEDDED_SUPPLEMENT: &[u8] = include_bytes!("../../assets/completion/supplement.json");
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

#[derive(Deserialize, Clone)]
pub struct Catalog {
    version: u32,
    pub roots: Vec<CommandNode>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct CommandNode {
    pub token: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub hidden: bool,
    pub flags: Vec<CommandFlag>,
    pub persistent: Vec<CommandFlag>,
    pub children: Vec<CommandNode>,
}
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
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
        let mut catalog = Self::decode(&decompress_catalog(EMBEDDED_CATALOG)?)?;
        catalog.merge_supplement(EMBEDDED_SUPPLEMENT)?;
        Ok(catalog)
    }

    /// Fills gaps in the generated catalog without touching its pinned bytes.
    /// A supplement root is added when absent, otherwise only the flags and
    /// subcommands the generated root lacks are appended.
    pub fn merge_supplement(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let supplement = Self::decode_unindexed(data)?;
        for root in supplement.roots {
            match self.index.get(&root.token).copied() {
                Some(existing) => {
                    let target = &mut self.roots[existing];
                    for flag in root.flags {
                        let token = flag.token();
                        if !target
                            .flags
                            .iter()
                            .chain(&target.persistent)
                            .any(|f| f.token() == token)
                        {
                            target.flags.push(flag);
                        }
                    }
                    for child in root.children {
                        if !target.children.iter().any(|c| c.token == child.token) {
                            target.children.push(child);
                        }
                    }
                }
                None => {
                    self.index.insert(root.token.clone(), self.roots.len());
                    self.roots.push(root);
                }
            }
        }
        for (i, root) in self.roots.iter().enumerate() {
            for alias in &root.aliases {
                self.index.entry(alias.clone()).or_insert(i);
            }
        }
        Ok(())
    }

    /// Merges a user-written spec file, where entries override the built-in
    /// ones: a flag with the same token replaces the built-in flag, a subcommand
    /// with the same token is merged recursively and a new one is added. The
    /// file is fully validated before the catalog is touched, so a rejected
    /// file leaves it unchanged. Descriptions are sanitized because they are
    /// shown in the popup.
    pub fn merge_user_spec(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let mut spec = Self::decode_unindexed(data)?;
        let mut nodes = 0usize;
        for root in &mut spec.roots {
            sanitize_user_node(root, &mut nodes);
        }
        if nodes > MAX_USER_SPEC_NODES {
            return Err("Completion spec has too many commands");
        }
        for root in spec.roots {
            match self.index.get(&root.token).copied() {
                Some(existing) => merge_node(&mut self.roots[existing], root),
                None => {
                    self.index.insert(root.token.clone(), self.roots.len());
                    self.roots.push(root);
                }
            }
        }
        for (i, root) in self.roots.iter().enumerate() {
            for alias in &root.aliases {
                self.index.entry(alias.clone()).or_insert(i);
            }
        }
        Ok(())
    }

    fn decode_unindexed(data: &[u8]) -> Result<Self, &'static str> {
        let catalog: Self =
            serde_json::from_slice(data).map_err(|_| "Invalid completion supplement")?;
        if catalog.version != 1 || !catalog.roots.iter().all(|r| Self::valid_node(r, 0)) {
            return Err("Invalid completion supplement");
        }
        Ok(catalog)
    }

    fn valid_node(node: &CommandNode, depth: usize) -> bool {
        depth <= 32
            && bare_word(&node.token)
            && node.aliases.iter().all(|a| bare_word(a))
            && node.flags.iter().chain(&node.persistent).all(|f| {
                (!f.long.is_empty() || !f.short.is_empty())
                    && (f.long.is_empty() || bare_word(&f.long))
                    && (f.short.is_empty() || bare_word(&f.short))
                    && f.values.iter().all(|v| bare_word(v))
            })
            && node.children.iter().all(|n| Self::valid_node(n, depth + 1))
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
        for (i, root) in catalog.roots.iter().enumerate() {
            if !Self::valid_node(root, 0) || catalog.index.insert(root.token.clone(), i).is_some() {
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

/// Upper bound on commands and subcommands in one user spec file.
pub const MAX_USER_SPEC_NODES: usize = 20_000;
const MAX_DESCRIPTION_CHARS: usize = 256;

fn sanitize_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_DESCRIPTION_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn sanitize_user_node(node: &mut CommandNode, nodes: &mut usize) {
    *nodes += 1;
    node.description = sanitize_text(&node.description);
    for flag in node.flags.iter_mut().chain(&mut node.persistent) {
        flag.description = sanitize_text(&flag.description);
    }
    for child in &mut node.children {
        sanitize_user_node(child, nodes);
    }
}

/// A flag with the same token replaces the existing one in place.
fn merge_flags(target: &mut Vec<CommandFlag>, source: Vec<CommandFlag>) {
    for flag in source {
        let token = flag.token();
        match target.iter_mut().find(|f| f.token() == token) {
            Some(existing) => *existing = flag,
            None => target.push(flag),
        }
    }
}

/// `source` wins on conflicts; nothing already present is removed.
fn merge_node(target: &mut CommandNode, source: CommandNode) {
    if !source.description.is_empty() {
        target.description = source.description;
    }
    target.hidden |= source.hidden;
    for alias in source.aliases {
        if alias != target.token && !target.aliases.contains(&alias) {
            target.aliases.push(alias);
        }
    }
    merge_flags(&mut target.flags, source.flags);
    merge_flags(&mut target.persistent, source.persistent);
    for child in source.children {
        match target.children.iter_mut().find(|c| c.token == child.token) {
            Some(existing) => merge_node(existing, child),
            None => target.children.push(child),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::completion::catalog::{
        Catalog, CommandNode, EMBEDDED_CATALOG, EMBEDDED_SUPPLEMENT, MAX_CATALOG_BYTES,
        decompress_catalog,
    };
    use crate::completion::parser::parse_context;
    use crate::completion::types::{CompletionScope, ShellDialect};
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::collections::HashSet;
    use std::io::Write;

    const ORIGINAL_CATALOG: &[u8] = include_bytes!("../../assets/completion/catalog.json");

    fn embedded_options(input: &str) -> Vec<String> {
        let mut context = parse_context(
            input,
            input.len(),
            ShellDialect::Bash,
            CompletionScope::legacy(),
        )
        .unwrap();
        Catalog::embedded()
            .unwrap()
            .candidates(&mut context)
            .into_iter()
            .map(|(text, _, _)| text)
            .collect()
    }

    #[test]
    fn flag_names_are_validated_before_any_catalog_mutation() {
        for group in ["flags", "persistent"] {
            for field in ["long", "short"] {
                for name in [
                    "bad\nflag",
                    "bad\u{1b}flag",
                    "bad\u{200b}flag",
                    "bad\u{202e}flag",
                ] {
                    let data = serde_json::to_vec(&serde_json::json!({"version": 1, "roots": [{"token": "tool", group: [{field: name}]}]})).unwrap();
                    assert!(super::Catalog::decode(&data).is_err());
                    let mut catalog = super::Catalog::embedded().unwrap();
                    let count = catalog.roots.len();
                    assert!(catalog.merge_user_spec(&data).is_err());
                    assert!(catalog.merge_supplement(&data).is_err());
                    assert_eq!(catalog.roots.len(), count);
                }
            }
        }
    }

    #[test]
    fn supplement_fills_commands_the_generated_catalog_lacks() {
        let ls = embedded_options("ls -");
        assert!(ls.iter().any(|t| t == "--all") && ls.iter().any(|t| t == "-l"));
        assert!(embedded_options("ip a").iter().any(|t| t == "addr"));
        assert!(embedded_options("yum ins").iter().any(|t| t == "install"));
        assert!(
            embedded_options("systemctl --")
                .iter()
                .any(|t| t == "--user")
        );
        assert!(embedded_options("ss -").iter().any(|t| t == "--listening"));
    }

    fn candidates_of(catalog: &Catalog, input: &str) -> Vec<(String, String)> {
        let mut context = parse_context(
            input,
            input.len(),
            ShellDialect::Bash,
            CompletionScope::legacy(),
        )
        .unwrap();
        catalog
            .candidates(&mut context)
            .into_iter()
            .map(|(text, description, _)| (text, description))
            .collect()
    }

    fn description_of(catalog: &Catalog, input: &str, token: &str) -> Option<String> {
        candidates_of(catalog, input)
            .into_iter()
            .find(|(text, _)| text == token)
            .map(|(_, description)| description)
    }

    #[test]
    fn user_spec_overrides_a_built_in_flag_and_adds_new_entries_without_duplicates() {
        let mut catalog = Catalog::embedded().unwrap();
        let before = candidates_of(&catalog, "ls -")
            .iter()
            .filter(|(text, _)| text == "--all")
            .count();
        assert_eq!(before, 1);
        let spec = br#"{"version":1,"roots":[
            {"token":"ls","description":"mine","flags":[
                {"long":"all","short":"a","description":"show everything"},
                {"long":"fancy","description":"my own flag"}]},
            {"token":"deploy","description":"ship it","children":[
                {"token":"staging","description":"to staging","flags":[{"long":"force"}]}]}
        ]}"#;
        catalog.merge_user_spec(spec).unwrap();
        let flags = candidates_of(&catalog, "ls -");
        assert_eq!(flags.iter().filter(|(text, _)| text == "--all").count(), 1);
        assert_eq!(
            description_of(&catalog, "ls -", "--all").as_deref(),
            Some("show everything")
        );
        assert!(flags.iter().any(|(text, _)| text == "--fancy"));
        assert!(flags.iter().any(|(text, _)| text == "-l"), "built-ins stay");
        assert_eq!(
            description_of(&catalog, "", "deploy").as_deref(),
            Some("ship it")
        );
        assert!(
            candidates_of(&catalog, "deploy ")
                .iter()
                .any(|(text, _)| text == "staging")
        );
        assert!(
            candidates_of(&catalog, "deploy staging --")
                .iter()
                .any(|(text, _)| text == "--force")
        );
    }

    #[test]
    fn the_documented_example_spec_is_valid_and_complete() {
        let mut catalog = Catalog::embedded().unwrap();
        let example = br#"{
  "version": 1,
  "roots": [
    {
      "token": "deploy",
      "description": "Ship a release",
      "children": [
        {
          "token": "staging",
          "description": "Deploy to staging",
          "flags": [
            { "long": "force", "short": "f", "description": "Skip checks" },
            { "long": "env", "takes_value": true, "values": ["blue", "green"] }
          ]
        }
      ]
    },
    {
      "token": "ls",
      "flags": [{ "long": "all", "short": "a", "description": "Show hidden entries" }]
    }
  ]
}"#;
        catalog.merge_user_spec(example).unwrap();
        assert_eq!(
            description_of(&catalog, "deploy staging --", "--force").as_deref(),
            Some("Skip checks")
        );
        let values = candidates_of(&catalog, "deploy staging --env ");
        assert!(values.iter().any(|(text, _)| text == "blue"));
        assert_eq!(
            description_of(&catalog, "ls --al", "--all").as_deref(),
            Some("Show hidden entries")
        );
    }

    #[test]
    fn user_spec_merges_subcommands_of_built_in_commands_recursively() {
        let mut catalog = Catalog::embedded().unwrap();
        let spec = br#"{"version":1,"roots":[{"token":"git","children":[
            {"token":"yolo","description":"push without asking"}]}]}"#;
        catalog.merge_user_spec(spec).unwrap();
        let subcommands = candidates_of(&catalog, "git ");
        assert!(subcommands.iter().any(|(text, _)| text == "yolo"));
        assert!(subcommands.iter().any(|(text, _)| text == "checkout"));
    }

    #[test]
    fn rejected_user_specs_leave_the_catalog_unchanged() {
        let mut catalog = Catalog::embedded().unwrap();
        let before = candidates_of(&catalog, "");
        for spec in [
            &b"not json"[..],
            br#"{"version":2,"roots":[]}"#,
            br#"{"version":1,"roots":[{"token":"bad token"}]}"#,
            br#"{"version":1,"roots":[{"token":"ok","flags":[{"description":"no name"}]}]}"#,
            br#"{"version":1,"roots":[{"token":"ok","flags":[{"long":"x","values":["a b"]}]}]}"#,
            &[0xff, 0xfe][..],
            &b""[..],
        ] {
            assert!(catalog.merge_user_spec(spec).is_err());
        }
        assert_eq!(candidates_of(&catalog, ""), before);
    }

    #[test]
    fn user_spec_descriptions_cannot_carry_terminal_controls_or_unbounded_text() {
        let mut catalog = Catalog::embedded().unwrap();
        let long = "x".repeat(5000);
        let spec = format!(
            r#"{{"version":1,"roots":[{{"token":"tool","description":"a\u001b[31mb\u0007","flags":[{{"long":"v","description":"{long}"}}]}}]}}"#
        );
        catalog.merge_user_spec(spec.as_bytes()).unwrap();
        assert_eq!(
            description_of(&catalog, "", "tool").as_deref(),
            Some("a[31mb")
        );
        let described = description_of(&catalog, "tool --", "--v").unwrap();
        assert_eq!(described.chars().count(), 256);
    }

    #[test]
    fn user_spec_with_too_many_commands_is_rejected() {
        let mut catalog = Catalog::embedded().unwrap();
        let roots: Vec<String> = (0..=super::MAX_USER_SPEC_NODES)
            .map(|n| format!(r#"{{"token":"c{n}"}}"#))
            .collect();
        let spec = format!(r#"{{"version":1,"roots":[{}]}}"#, roots.join(","));
        assert!(catalog.merge_user_spec(spec.as_bytes()).is_err());
        assert!(description_of(&catalog, "", "c0").is_none());
    }

    #[test]
    fn user_alias_never_steals_a_built_in_command_token() {
        let mut catalog = Catalog::embedded().unwrap();
        let spec = br#"{"version":1,"roots":[{"token":"mytool","aliases":["git"],
            "flags":[{"long":"mine"}]}]}"#;
        catalog.merge_user_spec(spec).unwrap();
        assert!(
            candidates_of(&catalog, "git ")
                .iter()
                .any(|(text, _)| text == "checkout")
        );
    }

    #[test]
    fn supplement_covers_representative_server_commands() {
        for (input, expected) in [
            ("iptables -", "--dport"),
            ("iptables -t ", "nat"),
            ("iptables -j ", "DROP"),
            ("nft ", "list"),
            ("firewall-cmd --", "--add-service"),
            ("crontab -", "-l"),
            ("nmap -", "-sV"),
            ("nmap -", "--top-ports"),
            ("tcpdump -", "-nn"),
            ("tcpdump -Q ", "inout"),
            ("lsof -", "-i"),
            ("sar -n ", "DEV"),
            ("iostat -", "-x"),
            ("yq ", "eval"),
            ("yq -o ", "json"),
            ("7z ", "x"),
            ("ansible -", "--inventory"),
            ("ansible-playbook -", "--syntax-check"),
            ("ansible-vault ", "encrypt"),
            ("ansible-galaxy ", "collection"),
            ("pacman -", "--sync"),
            ("strace -", "--follow-forks"),
            ("nc -", "-z"),
            ("dpkg -", "--configure"),
            ("python3 -", "-m"),
            ("reboot -", "--force"),
            ("uv ", "sync"),
            ("egrep --", "--after-context"),
            ("podman ", "run"),
            ("ethtool -", "-S"),
            ("timedatectl ", "set-timezone"),
            ("rpm -", "--queryformat"),
            ("zypper ", "install"),
            ("npm -", "--global"),
            ("brew --", "--prefix"),
            ("terraform -", "-chdir"),
            ("ufw --", "--dry-run"),
            ("blkid -o ", "export"),
            ("parted -a ", "optimal"),
            ("ip ", "route"),
            ("ss -", "--listening"),
        ] {
            assert!(
                embedded_options(input).iter().any(|text| text == expected),
                "{input:?} should offer {expected:?}"
            );
        }
    }

    #[test]
    fn supplement_has_no_duplicate_flags_or_children() {
        fn check(node: &CommandNode) {
            let mut flags = HashSet::new();
            for flag in node.flags.iter().chain(&node.persistent) {
                assert!(
                    flags.insert(flag.token()),
                    "{} repeats flag {}",
                    node.token,
                    flag.token()
                );
                if !flag.short.is_empty() && !flag.long.is_empty() {
                    assert!(
                        flags.insert(format!("-{}", flag.short)),
                        "{} repeats short flag -{}",
                        node.token,
                        flag.short
                    );
                }
            }
            let mut children = HashSet::new();
            for child in &node.children {
                assert!(
                    children.insert(child.token.as_str()),
                    "{} repeats subcommand {}",
                    node.token,
                    child.token
                );
                check(child);
            }
        }
        let supplement = Catalog::decode_unindexed(EMBEDDED_SUPPLEMENT).unwrap();
        let mut roots = HashSet::new();
        for root in &supplement.roots {
            assert!(
                roots.insert(root.token.as_str()),
                "repeated root {}",
                root.token
            );
            check(root);
        }
        assert!(supplement.roots.len() >= 70);
    }

    #[test]
    fn supplement_never_changes_a_flag_the_generated_catalog_already_has() {
        let generated = Catalog::decode(ORIGINAL_CATALOG).unwrap();
        let merged = Catalog::embedded().unwrap();
        for root in &generated.roots {
            let merged_root = merged
                .roots
                .iter()
                .find(|candidate| candidate.token == root.token)
                .unwrap();
            for flag in root.flags.iter().chain(&root.persistent) {
                let kept = merged_root
                    .flags
                    .iter()
                    .chain(&merged_root.persistent)
                    .find(|candidate| candidate.token() == flag.token())
                    .unwrap();
                assert_eq!(kept.description, flag.description, "{}", root.token);
                assert_eq!(kept.takes_value, flag.takes_value, "{}", root.token);
            }
        }
    }

    #[test]
    fn an_optional_value_flag_does_not_swallow_the_next_word() {
        let after_color = embedded_options("ls --color -");
        assert!(after_color.iter().any(|t| t == "--all"));
    }

    #[test]
    fn supplement_leaves_fully_generated_commands_unchanged() {
        let input = "git --";
        let mut context = parse_context(
            input,
            input.len(),
            ShellDialect::Bash,
            CompletionScope::legacy(),
        )
        .unwrap();
        let generated = Catalog::decode(ORIGINAL_CATALOG).unwrap();
        assert_eq!(
            Catalog::embedded()
                .unwrap()
                .candidates(&mut context.clone()),
            generated.candidates(&mut context)
        );
    }

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
        let embedded = Catalog::decode(&decompress_catalog(EMBEDDED_CATALOG).unwrap()).unwrap();
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
