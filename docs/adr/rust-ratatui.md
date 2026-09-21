# Rust + ratatui for the TUI

Built in Rust with ratatui. v2 dropped the hand-drawn map canvas for a
list-first view, but total control of the terminal still matters (tree guides,
right-aligned metadata, the persistent preview, and the launch modals), a
single static binary suits a long-lived personal tool, and
markdown/frontmatter/file-watching needs are all well-served by existing
crates. Go + Bubble Tea was the equal second choice (faster iteration, Lip Gloss
styling); Python + Textual was rejected for distribution friction despite the
fastest path to visual richness.
