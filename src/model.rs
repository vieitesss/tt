//! Task model: stable IDs, states, priorities, and the on-disk document
//! format (YAML frontmatter + markdown body).
//!
//! A task is stored as one markdown file per task. The frontmatter holds the
//! structured fields (`id`, `title`, `state`, `parent`, `tags`, `due`,
//! `priority`, `rank`); the body holds free markdown including untyped `[[id]]`
//! links.
//! The tree lives in the `parent` pointer, never in folder structure.
//!
//! Unknown frontmatter keys are preserved on round-trip so rewriting a file
//! that the user edited externally never drops their data. Line endings are
//! normalized to `\n` when a parsed document is written back.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Length, in characters, of freshly generated task IDs.
pub const GENERATED_ID_LEN: usize = 10;

/// Maximum accepted length of a task ID read from a document.
pub const MAX_ID_LEN: usize = 64;

/// Extension of every task file in a store. The stem is the Identity, so the
/// filename of a task is exactly [`TaskId::file_name`].
pub const TASK_EXTENSION: &str = "md";

/// Number of generation attempts before widening the candidate ID.
const MAX_GENERATE_ATTEMPTS: usize = 1_000;

/// Attempts between widening the candidate ID by one character.
const WIDEN_EVERY: usize = 100;

/// Alphabet used for generated task IDs. Restricted to filename-safe
/// characters that also work inside `[[...]]` links.
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Characters that may appear in an ID read from a document, on top of the
/// generated alphabet.
const ID_EXTRA_CHARS: [char; 2] = ['-', '_'];

fn valid_id_char(candidate: char) -> bool {
    candidate.is_ascii_digit()
        || candidate.is_ascii_lowercase()
        || ID_EXTRA_CHARS.contains(&candidate)
}

fn random_id(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ID_ALPHABET[rng.gen_range(0..ID_ALPHABET.len())] as char)
        .collect()
}

/// Reasons a candidate task ID was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    /// The ID was empty.
    #[error("task id must not be empty")]
    Empty,
    /// The ID exceeded [`MAX_ID_LEN`].
    #[error("task id is too long ({len} characters; maximum is {MAX_ID_LEN})")]
    TooLong {
        /// Observed length in characters.
        len: usize,
    },
    /// The ID contained a character outside `[0-9a-z_-]`.
    #[error("task id contains an invalid character {0:?}; allowed are [0-9a-z_-]")]
    InvalidChar(char),
}

/// A stable, filename-safe task identifier.
///
/// Generated IDs are always [`GENERATED_ID_LEN`] characters from `[0-9a-z]`.
/// Parsing is deliberately a little more permissive (up to [`MAX_ID_LEN`]
/// characters, `-`/`_` allowed) so hand-edited vaults keep working; anything
/// not filename-safe is rejected and reported by the vault as a skipped file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(String);

impl TaskId {
    /// Parse and validate an ID read from a document or supplied by a user.
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] when the value is empty, too long, or contains a
    /// character outside `[0-9a-z_-]`.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        if value.is_empty() {
            return Err(IdError::Empty);
        }
        let len = value.chars().count();
        if len > MAX_ID_LEN {
            return Err(IdError::TooLong { len });
        }
        if let Some(invalid) = value.chars().find(|c| !valid_id_char(*c)) {
            return Err(IdError::InvalidChar(invalid));
        }
        Ok(Self(value.to_owned()))
    }

    /// Generate a fresh, filename-safe task ID.
    ///
    /// `is_taken` is called with each candidate; generation retries until the
    /// predicate returns `false`. After [`MAX_GENERATE_ATTEMPTS`] collisions
    /// the candidate ID is widened.
    ///
    /// # Panics
    ///
    /// Panics if `is_taken` still reports every candidate as taken after
    /// [`MAX_GENERATE_ATTEMPTS`] attempts, which signals a broken collision
    /// check rather than a realistic vault.
    pub fn generate(mut is_taken: impl FnMut(&TaskId) -> bool) -> Self {
        for attempt in 0..MAX_GENERATE_ATTEMPTS {
            let len = GENERATED_ID_LEN + attempt / WIDEN_EVERY;
            let candidate = Self(random_id(len));
            if !is_taken(&candidate) {
                return candidate;
            }
        }
        panic!(
            "could not generate a free task id after {MAX_GENERATE_ATTEMPTS} attempts; \
             the collision check may be broken"
        );
    }

    /// Borrow the ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The task's store filename: the Identity stem plus [`TASK_EXTENSION`].
    ///
    /// The store is flat, so joining this onto a store root always yields the
    /// one file that holds the task.
    pub fn file_name(&self) -> String {
        format!("{}.{TASK_EXTENSION}", self.0)
    }

    /// Parse the target portion of a `[[...]]` wikilink into a task id.
    ///
    /// The store is flat and identity is the file stem, so a link target is
    /// normalized before validation: surrounding whitespace is trimmed, one
    /// trailing `.md` is stripped (case-sensitive), and any directory
    /// components are dropped. `[[abc.md]]`, `[[./abc.md]]`, and
    /// `[[sub/abc.md]]` all resolve to `abc`; returns `None` when the
    /// remaining stem is not a valid [`TaskId`].
    pub fn parse_link_target(target: &str) -> Option<Self> {
        let trimmed = target.trim();
        let without_suffix = trimmed
            .strip_suffix(TASK_EXTENSION)
            .and_then(|stem| stem.strip_suffix('.'))
            .unwrap_or(trimmed);
        let stem = without_suffix
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default();
        Self::parse(stem).ok()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for TaskId {
    type Err = IdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for TaskId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Not finished yet.
    #[default]
    Open,
    /// Finished.
    Done,
    /// Deliberately abandoned.
    Cancelled,
}

impl TaskState {
    /// Lowercase wire/storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    /// Flip `open`/`done`, leaving `cancelled` untouched (the TUI `x` action).
    pub const fn toggle_done(self) -> Self {
        match self {
            Self::Open => Self::Done,
            Self::Done => Self::Open,
            Self::Cancelled => Self::Cancelled,
        }
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Optional priority of a task. Declaration order runs highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Do first.
    High,
    /// Normal.
    Med,
    /// Do last.
    Low,
}

impl Priority {
    /// Lowercase wire/storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Med => "med",
            Self::Low => "low",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A single task, stored as one markdown file in the vault.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Stable identifier and the file stem: the identity the vault keys by.
    pub id: TaskId,
    /// The `id` written in the document frontmatter, normally equal to `id`.
    /// A file whose stem and frontmatter id differ still loads under the
    /// stem; the declared id is reported as an issue and preserved verbatim
    /// on rewrite so tt never silently "fixes" a user's file.
    pub(crate) declared_id: TaskId,
    /// Human-readable title.
    pub title: String,
    /// Lifecycle state.
    pub state: TaskState,
    /// Parent task, if this task is a sub-task. The tree lives here.
    pub parent: Option<TaskId>,
    /// Hierarchical tags without a leading `#`, stored exactly as written
    /// (`work/admin` implies `work`; implication is applied at query time).
    pub tags: Vec<String>,
    /// Due date, date only.
    pub due: Option<NaiveDate>,
    /// Optional priority.
    pub priority: Option<Priority>,
    /// Optional manual place among siblings, lower first.
    pub rank: Option<i32>,
    /// Free markdown body; may contain untyped `[[id]]` links.
    pub body: String,
    /// Unknown frontmatter keys, preserved so external edits round-trip.
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl Task {
    /// Create an open task with no parent, tags, dates, priority, rank, or body.
    pub fn new(id: TaskId, title: impl Into<String>) -> Self {
        Self {
            declared_id: id.clone(),
            id,
            title: title.into(),
            state: TaskState::Open,
            parent: None,
            tags: Vec::new(),
            due: None,
            priority: None,
            rank: None,
            body: String::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Parse a `---`-delimited YAML frontmatter document plus markdown body.
    ///
    /// A frontmatter block that parses but declares no `id` is a foreign
    /// note (for example a skill or an editor-side document), not a task; it
    /// is reported as [`ParseError::ForeignFrontmatter`] so callers can skip
    /// it silently.
    ///
    /// The vault reports parse failures as skipped files; it never rewrites a
    /// document it could not parse.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the document has no frontmatter block, the
    /// block is unterminated, the frontmatter declares no `id`, or the YAML is
    /// invalid or missing/invalid required fields (`id`, `title`, `state`).
    pub fn from_document(text: &str) -> Result<Self, ParseError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);

        let (first_line, after_open) = match text.split_once('\n') {
            Some((line, rest)) => (line.trim_end_matches('\r'), rest),
            None => (text.trim_end_matches('\r'), ""),
        };
        if first_line.trim_end() != "---" {
            return Err(ParseError::MissingFrontmatter);
        }

        // Find the closing `---` line and remember where the body starts.
        let mut yaml_end = None;
        let mut body_start = 0usize;
        for line in after_open.split_inclusive('\n') {
            let without_eol = line.trim_end_matches(['\r', '\n']);
            body_start += line.len();
            if without_eol.trim_end() == "---" {
                yaml_end = Some(body_start - line.len());
                break;
            }
        }
        let yaml_end = yaml_end.ok_or(ParseError::UnterminatedFrontmatter)?;

        // Parse the block generically first: a block without an `id` key is a
        // foreign note, while a block that claims an `id` is a task and any
        // further failure is a broken task, not a note.
        let yaml = &after_open[..yaml_end];
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        if value.get("id").is_none() {
            return Err(ParseError::ForeignFrontmatter);
        }

        let frontmatter: Frontmatter = serde_yaml::from_str(yaml)?;
        Ok(Self {
            declared_id: frontmatter.id.clone(),
            id: frontmatter.id,
            title: frontmatter.title,
            state: frontmatter.state,
            parent: frontmatter.parent,
            tags: normalize_tags(frontmatter.tags),
            due: frontmatter.due,
            priority: frontmatter.priority,
            rank: frontmatter.rank,
            body: after_open[body_start..].to_owned(),
            extra: frontmatter.extra,
        })
    }

    /// Serialize to the on-disk `---`-delimited document.
    ///
    /// Unknown frontmatter keys are written back after the known fields.
    pub fn to_document(&self) -> String {
        let frontmatter = Frontmatter::from(self);
        let yaml = serde_yaml::to_string(&frontmatter)
            .expect("task frontmatter always serializes to YAML")
            .trim_end_matches(['\r', '\n'])
            .to_owned();

        let mut document = String::with_capacity(yaml.len() + self.body.len() + 16);
        document.push_str("---\n");
        document.push_str(&yaml);
        document.push_str("\n---\n");
        document.push_str(&self.body);
        document
    }
}

/// Errors from [`Task::from_document`].
#[derive(Debug, Error)]
pub enum ParseError {
    /// The document does not open with a `---` frontmatter block.
    #[error("document does not start with a `---` YAML frontmatter block")]
    MissingFrontmatter,
    /// The frontmatter block never closes with a `---` line.
    #[error("YAML frontmatter block is missing its closing `---` line")]
    UnterminatedFrontmatter,
    /// The frontmatter block parses but carries no task `id`; the file is a
    /// foreign note, not a task, and callers should skip it silently.
    #[error("YAML frontmatter has no task `id`; not a task document")]
    ForeignFrontmatter,
    /// The YAML is malformed or missing a required field.
    #[error("invalid YAML frontmatter: {0}")]
    Frontmatter(#[from] serde_yaml::Error),
}

/// Serde representation of the YAML frontmatter block.
#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    id: TaskId,
    title: String,
    state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rank: Option<i32>,
    /// Unrecognized keys, preserved verbatim.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

impl From<&Task> for Frontmatter {
    fn from(task: &Task) -> Self {
        Self {
            id: task.declared_id.clone(),
            title: task.title.clone(),
            state: task.state,
            parent: task.parent.clone(),
            tags: task.tags.clone(),
            due: task.due,
            priority: task.priority,
            rank: task.rank,
            extra: task.extra.clone(),
        }
    }
}

/// Trim tags, drop leading `#`, drop empties, and de-duplicate in order.
pub(crate) fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let cleaned = tag.trim().trim_start_matches('#').trim();
        if cleaned.is_empty() || normalized.iter().any(|existing| existing == cleaned) {
            continue;
        }
        normalized.push(cleaned.to_owned());
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> TaskId {
        TaskId::parse(value).expect("valid test id")
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid test date")
    }

    fn sample_task() -> Task {
        Task {
            declared_id: id("abc1234567"),
            id: id("abc1234567"),
            title: "Ship \"M1\" — colons: yes".to_owned(),
            state: TaskState::Done,
            parent: Some(id("parent00000")),
            tags: vec!["work/admin".to_owned(), "home".to_owned()],
            due: Some(date(2026, 1, 2)),
            priority: Some(Priority::High),
            rank: Some(3),
            body: "# Heading\n\nSee [[abc1234567]] and `inline code`.\n\n- [ ] item\n".to_owned(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn fully_populated_task_roundtrips() {
        let task = sample_task();
        let document = task.to_document();
        let parsed = Task::from_document(&document).expect("valid document");

        assert_eq!(parsed, task);
        assert_eq!(parsed.to_document(), document, "serialization is stable");
    }

    #[test]
    fn omitted_optional_fields_default() {
        let document = "---\nid: abc1234567\ntitle: Minimal\nstate: open\n---\n";
        let task = Task::from_document(document).expect("valid document");

        assert_eq!(task.parent, None);
        assert!(task.tags.is_empty());
        assert_eq!(task.due, None);
        assert_eq!(task.priority, None);
        assert_eq!(task.rank, None);
        assert_eq!(task.body, "");
        assert!(task.extra.is_empty());
        assert_eq!(task.state, TaskState::Open);
    }

    #[test]
    fn rank_roundtrips_and_none_is_omitted() {
        let mut task = Task::new(id("abc1234567"), "Ranked");
        assert!(!task.to_document().contains("rank:"));

        task.rank = Some(4);
        let document = task.to_document();
        assert!(document.contains("rank: 4"));
        assert_eq!(Task::from_document(&document).expect("parse").rank, Some(4));
    }

    #[test]
    fn unknown_frontmatter_keys_survive_roundtrip() {
        let document = "---\nid: abc1234567\ntitle: T\nstate: open\ncustom: 42\nnested:\n  a: [1, 2]\n---\nbody\n";
        let task = Task::from_document(document).expect("valid document");

        assert!(task.extra.contains_key("custom"));
        assert!(task.extra.contains_key("nested"));

        let reparsed = Task::from_document(&task.to_document()).expect("valid document");
        assert_eq!(reparsed, task);
        assert!(task.to_document().contains("custom: 42"));
    }

    #[test]
    fn generated_ids_are_ten_filename_safe_chars() {
        for _ in 0..100 {
            let generated = TaskId::generate(|_| false);
            assert_eq!(generated.as_str().chars().count(), GENERATED_ID_LEN);
            assert!(generated
                .as_str()
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()));
            assert!(TaskId::parse(generated.as_str()).is_ok());
        }
    }

    #[test]
    fn generate_retries_when_id_is_taken() {
        let mut attempts = 0;
        let generated = TaskId::generate(|_| {
            attempts += 1;
            attempts <= 3
        });

        assert_eq!(attempts, 4, "generation should retry while ids are taken");
        assert!(TaskId::parse(generated.as_str()).is_ok());
    }

    #[test]
    fn invalid_state_is_rejected() {
        let document = "---\nid: abc1234567\ntitle: T\nstate: almost\n---\n";
        let error = Task::from_document(document).expect_err("invalid state must fail");
        assert!(matches!(error, ParseError::Frontmatter(_)));
    }

    #[test]
    fn invalid_priority_is_rejected() {
        let document = "---\nid: abc1234567\ntitle: T\nstate: open\npriority: urgent\n---\n";
        let error = Task::from_document(document).expect_err("invalid priority must fail");
        assert!(matches!(error, ParseError::Frontmatter(_)));
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        // A document that claims an `id` is a task; missing or invalid other
        // fields make it a broken task, not a note.
        let documents = [
            "---\nid: abc1234567\nstate: open\n---\n",
            "---\nid: abc1234567\ntitle: T\n---\n",
        ];
        for document in documents {
            assert!(
                matches!(
                    Task::from_document(document),
                    Err(ParseError::Frontmatter(_))
                ),
                "document should be rejected: {document:?}"
            );
        }
    }

    #[test]
    fn frontmatter_without_an_id_is_a_foreign_note() {
        let documents = [
            "---\ntitle: T\nstate: open\n---\n",
            "---\ntitle: A note\ntags: [work]\n---\nbody\n",
            "---\n---\n",
            "---\n- a\n- b\n---\n",
        ];
        for document in documents {
            assert!(
                matches!(
                    Task::from_document(document),
                    Err(ParseError::ForeignFrontmatter)
                ),
                "document should be a foreign note: {document:?}"
            );
        }
    }

    #[test]
    fn missing_frontmatter_is_rejected() {
        for document in ["", "id: abc1234567\ntitle: T\nstate: open\n"] {
            assert!(matches!(
                Task::from_document(document),
                Err(ParseError::MissingFrontmatter)
            ));
        }
    }

    #[test]
    fn unterminated_frontmatter_is_rejected() {
        let document = "---\nid: abc1234567\ntitle: T\nstate: open\n";
        assert!(matches!(
            Task::from_document(document),
            Err(ParseError::UnterminatedFrontmatter)
        ));
    }

    #[test]
    fn body_is_preserved_verbatim() {
        let body = "first\n\n---\n\n```rust\nlet x = 1;\n```\nlast\n";
        let document = format!("---\nid: abc1234567\ntitle: T\nstate: open\n---\n{body}");
        let task = Task::from_document(&document).expect("valid document");

        assert_eq!(task.body, body);
        assert_eq!(task.to_document(), document);
    }

    #[test]
    fn crlf_documents_parse() {
        let document = "---\r\nid: abc1234567\r\ntitle: T\r\nstate: open\r\n---\r\nline\r\n";
        let task = Task::from_document(document).expect("valid document");

        assert_eq!(task.title, "T");
        assert_eq!(task.body, "line\r\n");
    }

    #[test]
    fn leading_byte_order_mark_is_tolerated() {
        let document = "\u{feff}---\nid: abc1234567\ntitle: T\nstate: open\n---\nbody\n";
        let task = Task::from_document(document).expect("valid document");
        assert_eq!(task.body, "body\n");
    }

    #[test]
    fn tags_are_normalized_and_deduplicated() {
        let document = "---\nid: abc1234567\ntitle: T\nstate: open\ntags:\n  - \"#work\"\n  - \" work \"\n  - work\n  - \"\"\n---\n";
        let task = Task::from_document(document).expect("valid document");

        assert_eq!(task.tags, vec!["work".to_owned()]);
    }

    #[test]
    fn task_id_rejects_unsafe_values() {
        assert_eq!(TaskId::parse(""), Err(IdError::Empty));
        assert_eq!(TaskId::parse("abc/def"), Err(IdError::InvalidChar('/')));
        assert_eq!(TaskId::parse("UPPER"), Err(IdError::InvalidChar('U')));
        assert!(matches!(
            TaskId::parse(&"a".repeat(MAX_ID_LEN + 1)),
            Err(IdError::TooLong { .. })
        ));
        assert!(TaskId::parse("my_task-1").is_ok());
    }

    #[test]
    fn link_targets_normalize_to_the_filename_stem() {
        for (target, expected) in [
            ("abc1234567", "abc1234567"),
            ("  abc1234567  ", "abc1234567"),
            ("abc1234567.md", "abc1234567"),
            ("sub/dir/abc1234567.md", "abc1234567"),
            ("sub\\abc1234567", "abc1234567"),
            ("./abc1234567.md", "abc1234567"),
        ] {
            assert_eq!(
                TaskId::parse_link_target(target).map(|id| id.as_str().to_owned()),
                Some(expected.to_owned()),
                "target {target:?}"
            );
        }

        for target in [
            "",
            ".md",
            "abc1234567.md.md",
            "UPPER.md",
            "not a link",
            "a b.md",
        ] {
            assert!(
                TaskId::parse_link_target(target).is_none(),
                "target {target:?} must not resolve"
            );
        }
    }

    #[test]
    fn toggle_done_leaves_cancelled_alone() {
        assert_eq!(TaskState::Open.toggle_done(), TaskState::Done);
        assert_eq!(TaskState::Done.toggle_done(), TaskState::Open);
        assert_eq!(TaskState::Cancelled.toggle_done(), TaskState::Cancelled);
    }
}
