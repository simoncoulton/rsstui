use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{tags, theme::ThemeSpec};

const DEFAULT_CONFIG: &str = r##"# rsstui configuration.
# Everything here is optional; delete a line to get its default.
# Press E in the app to edit this file; it reloads when the editor exits.

# Draw each article's lead image.
images: true

# Refresh all feeds every N minutes (0 disables auto-refresh).
refresh_minutes: 0

# "auto" detects kitty / iTerm2 / sixel support. Use "halfblocks" if your
# terminal does not answer capability queries (input can stall otherwise).
image_protocol: auto

# Colour theme. Themes are the files in ~/.config/rsstui/themes/*.yml (a file
# is written for each built-in on first run; edit them or add your own and they
# are picked up the next time you press T). Press T in the app to preview and
# pick one; the choice is saved here.
theme: default

# Show the terminal's own background instead of the theme's page colour.
transparent_background: true

# How the Items and Article panes sit: horizontal (side by side) or vertical
# (items above the article). Press v in the app to toggle; the choice is saved.
layout: horizontal

# Your own themes, as an alternative to theme files. Every colour is optional
# and inherits from `base` (one of the built-in themes, default: `default`).
#
# themes:
#   my-theme:
#     base: dracula
#     accent: "#ff79c6"

# Feeds. Press a in the app to add one, or list them here. `folder` groups
# feeds in the Feeds pane; `tags` are for filtering (press f). The app rewrites
# this block when you add, remove, tag or move a feed; comments elsewhere in the
# file are kept.
#
#   - name: Example Blog          # optional: filled in from the feed's title
#     url: https://example.com/feed.xml
#     folder: Reading             # optional
#     tags: [example]             # optional
feeds: []
"##;

fn yes() -> bool {
    true
}

fn auto() -> String {
    "auto".into()
}

fn default_theme() -> String {
    "default".into()
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FeedConfig {
    #[serde(default)]
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl FeedConfig {
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.url
        } else {
            &self.name
        }
    }
}

/// How the Items and Article panes sit next to each other.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Side by side.
    #[default]
    Horizontal,
    /// Items above the article.
    Vertical,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::Horizontal => "horizontal",
            Layout::Vertical => "vertical",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Layout::Horizontal => Layout::Vertical,
            Layout::Vertical => Layout::Horizontal,
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "yes")]
    pub images: bool,
    #[serde(default)]
    pub refresh_minutes: u64,
    #[serde(default = "auto")]
    pub image_protocol: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "yes")]
    pub transparent_background: bool,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub themes: BTreeMap<String, ThemeSpec>,
    #[serde(default)]
    pub feeds: Vec<FeedConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            images: true,
            refresh_minutes: 0,
            image_protocol: auto(),
            theme: default_theme(),
            transparent_background: true,
            layout: Layout::default(),
            themes: BTreeMap::new(),
            feeds: Vec::new(),
        }
    }
}

/// Where everything lives. `~/.config/rsstui` (on macOS too), not the
/// platform-native config directory.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub themes: PathBuf,
}

impl Paths {
    pub fn user() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(Self::in_dir(
            &PathBuf::from(home).join(".config").join("rsstui"),
        ))
    }

    pub fn in_dir(dir: &Path) -> Self {
        Self {
            config: dir.join("config.yml"),
            state: dir.join("state.yml"),
            themes: dir.join("themes"),
        }
    }
}

/// Write via a temporary file and a rename, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// Parse and validate. Tags are normalised; unknown keys are errors.
pub fn parse(text: &str) -> Result<Config> {
    let mut cfg: Config = if text
        .lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
    {
        Config::default()
    } else {
        serde_yaml_ng::from_str(text)?
    };
    let mut seen = BTreeSet::new();
    for f in &mut cfg.feeds {
        if f.url.trim().is_empty() {
            bail!("a feed has an empty url");
        }
        if !seen.insert(f.url.clone()) {
            bail!("feed {} is listed twice", f.url);
        }
        f.tags = tags::parse_tags(&f.tags.join(" "))
            .map_err(|e| anyhow::anyhow!("feed {}: {e}", f.label()))?;
        if let Some(folder) = &f.folder {
            f.folder = Some(
                tags::parse_folder(folder)
                    .map_err(|e| anyhow::anyhow!("feed {}: {e}", f.label()))?,
            );
        }
    }
    Ok(cfg)
}

impl Config {
    /// Load the config, writing the documented default on first run. Errors
    /// name the file and, where YAML provides them, the line and column.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            write_atomic(path, DEFAULT_CONFIG)?;
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        parse(&text).with_context(|| format!("in {}", path.display()))
    }
}

// ---- in-place edits ------------------------------------------------------
//
// The config is hand-edited and commented, so it is never re-serialised as a
// whole: each edit replaces one top-level key or block and leaves every other
// line, comments included, alone.

fn read_for_edit(path: &Path) -> Result<String> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&text).with_context(|| {
        format!(
            "{} has an error, so it was not changed (fix it first, e.g. with E)",
            path.display()
        )
    })?;
    Ok(text)
}

/// A string as a YAML scalar, quoted if it needs it.
pub fn yaml_scalar(s: &str) -> String {
    serde_yaml_ng::to_string(s)
        .map(|t| t.trim_end().to_string())
        .unwrap_or_else(|_| format!("{s:?}"))
}

/// A string as an item of a YAML flow list (`[a, b]`): plain when that reads
/// back unchanged, else quoted.
fn flow_scalar(s: &str) -> String {
    let reads_back = |text: &str| {
        serde_yaml_ng::from_str::<Vec<String>>(&format!("[{text}]"))
            .is_ok_and(|v| v.len() == 1 && v[0] == s)
    };
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "_./~=:+@%-".contains(c))
        && reads_back(s);
    if plain {
        return s.to_string();
    }
    let single = format!("'{}'", s.replace('\'', "''"));
    if reads_back(&single) {
        return single;
    }
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

fn replace_key(text: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}:");
    let mut replaced = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|line| {
            if !replaced && line.starts_with(&prefix) {
                replaced = true;
                // Keep a trailing comment (and the spacing before it), if any.
                let rest = &line[prefix.len()..];
                match rest.find(" #") {
                    Some(i) => format!("{key}: {value}{}", &rest[rest[..i].trim_end().len()..]),
                    None => format!("{key}: {value}"),
                }
            } else {
                line.to_string()
            }
        })
        .collect();
    if !replaced {
        out.push(format!("{key}: {value}"));
    }
    out.join("\n") + "\n"
}

/// Set a top-level `key: value` (`value` is raw YAML), keeping every other
/// line. The result is checked to still parse before anything is written.
pub fn set_value(path: &Path, key: &str, value: &str) -> Result<()> {
    let existing = read_for_edit(path)?;
    let updated = replace_key(&existing, key, value);
    parse(&updated).with_context(|| format!("refusing to write invalid {key}: {value}"))?;
    write_atomic(path, &updated)
}

fn render_feeds(feeds: &[FeedConfig]) -> String {
    if feeds.is_empty() {
        return "feeds: []".into();
    }
    let mut out = String::from("feeds:\n");
    for f in feeds {
        let mut first = true;
        let mut line = |text: String| {
            out.push_str(if first { "  - " } else { "    " });
            out.push_str(&text);
            out.push('\n');
            first = false;
        };
        if !f.name.is_empty() {
            line(format!("name: {}", yaml_scalar(&f.name)));
        }
        line(format!("url: {}", yaml_scalar(&f.url)));
        if let Some(folder) = &f.folder {
            line(format!("folder: {}", yaml_scalar(folder)));
        }
        if !f.tags.is_empty() {
            let list = f
                .tags
                .iter()
                .map(|t| flow_scalar(t))
                .collect::<Vec<_>>()
                .join(", ");
            line(format!("tags: [{list}]"));
        }
    }
    out.trim_end().to_string()
}

fn replace_block(text: &str, key: &str, block: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.starts_with(&format!("{key}:")));
    let mut out: Vec<&str> = Vec::new();
    match start {
        Some(s) => {
            // The block runs while lines are indented or list items; blank
            // lines and comments only belong to it if more block follows.
            let mut end = s + 1;
            for (i, l) in lines.iter().enumerate().skip(s + 1) {
                if l.starts_with(char::is_whitespace) && !l.trim().is_empty()
                    || *l == "-"
                    || l.starts_with("- ")
                {
                    end = i + 1;
                } else if !l.trim().is_empty() && !l.starts_with('#') {
                    break;
                }
            }
            out.extend(&lines[..s]);
            out.extend(block.lines());
            out.extend(&lines[end..]);
        }
        None => {
            out.extend(&lines);
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push("");
            }
            out.extend(block.lines());
        }
    }
    out.join("\n") + "\n"
}

/// Write `feeds` as the config's `feeds:` block, leaving everything else as it
/// was. Refuses anything that would not read back identically.
pub fn set_feeds(path: &Path, feeds: &[FeedConfig]) -> Result<()> {
    let existing = read_for_edit(path)?;
    let updated = replace_block(&existing, "feeds", &render_feeds(feeds));
    let cfg = parse(&updated).context("refusing to write an invalid feeds list")?;
    if cfg.feeds != feeds {
        bail!("refusing to write feeds: they would not read back the same");
    }
    write_atomic(path, &updated)
}

// ---- state -----------------------------------------------------------------

/// Read-item tracking and UI memory, kept apart from the hand-edited config.
#[derive(Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub read: BTreeSet<String>,
    /// Folders collapsed in the Feeds pane.
    #[serde(default)]
    pub collapsed: BTreeSet<String>,
}

impl State {
    pub fn load(path: &Path) -> Self {
        if let Some(s) = fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_yaml_ng::from_str(&t).ok())
        {
            return s;
        }
        // Earlier versions kept the read list in state.json next to it.
        #[derive(Deserialize)]
        struct Old {
            read: BTreeSet<String>,
        }
        fs::read_to_string(path.with_file_name("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Old>(&t).ok())
            .map(|o| Self {
                read: o.read,
                collapsed: BTreeSet::new(),
            })
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_yaml_ng::to_string(self)?)
    }
}

/// The default config with the sample feeds in `docs/demo-feeds.yml`, for tests
/// and the demo recording. The shipped default has no feeds.
#[cfg(test)]
pub fn sample_config_text() -> String {
    DEFAULT_CONFIG.replace(
        "feeds: []",
        include_str!("../docs/demo-feeds.yml").trim_end(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsstui-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yml");
        fs::write(&path, body).unwrap();
        path
    }

    fn feed(url: &str, folder: Option<&str>, tags: &[&str]) -> FeedConfig {
        FeedConfig {
            name: String::new(),
            url: url.into(),
            folder: folder.map(Into::into),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn default_template_parses_to_defaults_with_no_feeds() {
        let c = parse(DEFAULT_CONFIG).unwrap();
        let d = Config::default();
        assert_eq!(
            (c.images, c.refresh_minutes, &c.image_protocol),
            (d.images, d.refresh_minutes, &d.image_protocol)
        );
        assert_eq!(
            (c.theme.as_str(), c.transparent_background, c.layout),
            ("default", true, Layout::Horizontal)
        );
        assert!(c.feeds.is_empty(), "no feeds are hardcoded");
    }

    #[test]
    fn sample_feeds_use_folders_and_tags() {
        let c = parse(&sample_config_text()).unwrap();
        assert!(c.feeds.len() >= 10);
        let folders: BTreeSet<_> = c.feeds.iter().filter_map(|f| f.folder.clone()).collect();
        assert!(folders.len() >= 3, "{folders:?}");
        assert!(c.feeds.iter().all(|f| !f.tags.is_empty()));
    }

    #[test]
    fn adding_the_first_feed_to_the_empty_default_works() {
        let path = temp("first", DEFAULT_CONFIG);
        set_feeds(&path, &[feed("https://a.test/f", None, &[])]).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(
            out.contains("# Feeds. Press a in the app"),
            "comments kept:\n{out}"
        );
        assert_eq!(parse(&out).unwrap().feeds.len(), 1);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_errors() {
        assert!(parse("them: nord\n").is_err());
        assert!(parse("feeds:\n  - url: u\n    nmae: x\n").is_err());
        assert!(parse("feeds:\n  - url: u\n    tags: ['a b!']\n").is_err());
        assert!(
            parse("feeds:\n  - url: u\n  - url: u\n")
                .unwrap_err()
                .to_string()
                .contains("twice")
        );
    }

    #[test]
    fn tags_are_normalised_on_load() {
        let c = parse("feeds:\n  - url: u\n    tags: [Rust, '#News']\n").unwrap();
        assert_eq!(c.feeds[0].tags, ["news", "rust"]);
    }

    #[test]
    fn set_feeds_rewrites_only_the_feeds_block() {
        let body = "# my notes\nimages: true # keep\n\n# feeds below\nfeeds:\n  # old comment\n  - url: https://a.test/f\n\n# after\ntheme: nord\n";
        let path = temp("feeds", body);
        let feeds = vec![
            FeedConfig {
                name: "A: b".into(),
                ..feed("https://a.test/f", Some("Dev & Ops"), &["x", "y-z"])
            },
            feed("https://b.test/f", None, &[]),
        ];
        set_feeds(&path, &feeds).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        for kept in [
            "# my notes",
            "images: true # keep",
            "# feeds below",
            "# after",
            "theme: nord",
        ] {
            assert!(out.contains(kept), "lost {kept:?} in:\n{out}");
        }
        assert_eq!(parse(&out).unwrap().feeds, feeds, "{out}");
        assert_eq!(parse(&out).unwrap().theme, "nord");
        set_feeds(&path, &[]).unwrap();
        assert!(
            parse(&fs::read_to_string(&path).unwrap())
                .unwrap()
                .feeds
                .is_empty()
        );
    }

    #[test]
    fn set_value_keeps_trailing_comments_and_adds_missing_keys() {
        let path = temp("value", "theme: default # pick\nfeeds: []\n");
        set_value(&path, "theme", "nord").unwrap();
        set_value(&path, "images", "false").unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("theme: nord # pick"), "{out}");
        let c = parse(&out).unwrap();
        assert_eq!((c.theme.as_str(), c.images), ("nord", false));
    }

    #[test]
    fn refuses_invalid_values_and_broken_files() {
        let path = temp("refuse", "images: true\n");
        assert!(set_value(&path, "images", "maybe").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "images: true\n");
        let path = temp("broken", "images: [\n");
        let err = set_value(&path, "theme", "nord").unwrap_err();
        assert!(format!("{err:#}").contains("has an error"), "{err:#}");
    }

    #[test]
    fn awkward_strings_survive_a_round_trip() {
        let path = temp("quote", "feeds: []\n");
        let feeds = vec![FeedConfig {
            name: "it's: \"odd\" # name".into(),
            ..feed(
                "https://x.test/a?b=1&c=2",
                Some("a: b"),
                &["4k", "a/b", "true"],
            )
        }];
        set_feeds(&path, &feeds).unwrap();
        assert_eq!(
            parse(&fs::read_to_string(&path).unwrap()).unwrap().feeds,
            feeds
        );
    }

    #[test]
    fn state_round_trips_and_imports_the_old_json() {
        let path = temp("state", "").with_file_name("state.yml");
        let mut s = State::default();
        s.read.insert("a|1".into());
        s.collapsed.insert("News".into());
        s.save(&path).unwrap();
        let back = State::load(&path);
        assert!(back.read.contains("a|1") && back.collapsed.contains("News"));

        let other = path.with_file_name("sub").join("state.yml");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(other.with_file_name("state.json"), r#"{"read":["x|y"]}"#).unwrap();
        assert!(State::load(&other).read.contains("x|y"));
    }
}
