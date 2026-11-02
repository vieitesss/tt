package vault

import (
	"bytes"
	"crypto/rand"
	"errors"
	"fmt"
	"math/big"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"go.yaml.in/yaml/v3"
)

var (
	ErrMissingFrontmatter      = errors.New("document does not start with a `---` YAML frontmatter block")
	ErrUnterminatedFrontmatter = errors.New("YAML frontmatter block is missing its closing `---` line")
	ErrForeignFrontmatter      = errors.New("YAML frontmatter has no task `id`; not a task document")
)

// Task is the in-memory form of one markdown task. ID is the file stem;
// DeclaredID is preserved even if it disagrees with that stem.
type Task struct {
	ID         string                `json:"id"`
	Title      string                `json:"title"`
	State      string                `json:"state"`
	Parent     *string               `json:"parent"`
	Tags       []string              `json:"tags"`
	Due        *string               `json:"due"`
	Priority   *string               `json:"priority"`
	Rank       *int32                `json:"rank"`
	DeclaredID string                `json:"-"`
	Body       string                `json:"-"`
	Extra      map[string]*yaml.Node `json:"-"`
}

func NewTask(id, title string) Task {
	return Task{ID: id, DeclaredID: id, Title: title, State: "open", Tags: []string{}, Extra: map[string]*yaml.Node{}}
}

func ValidateID(id string) error {
	if id == "" {
		return errors.New("task id must not be empty")
	}
	if n := utf8.RuneCountInString(id); n > 64 {
		return fmt.Errorf("task id is too long (%d characters; maximum is 64)", n)
	}
	for _, c := range id {
		if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '_') {
			return fmt.Errorf("task id contains an invalid character %q; allowed are [0-9a-z_-]", c)
		}
	}
	return nil
}

func generateID(taken func(string) bool) (string, error) {
	const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
	for attempt := 0; attempt < 1000; attempt++ {
		b := make([]byte, 10+attempt/100)
		for i := range b {
			n, err := rand.Int(rand.Reader, big.NewInt(int64(len(alphabet))))
			if err != nil {
				return "", err
			}
			b[i] = alphabet[n.Int64()]
		}
		id := string(b)
		if !taken(id) {
			return id, nil
		}
	}
	return "", errors.New("could not generate a free task id after 1000 attempts")
}

func ValidateDate(date string) error {
	if len(date) != len("2006-01-02") {
		return fmt.Errorf("invalid due date %q, expected YYYY-MM-DD", date)
	}
	parsed, err := time.Parse("2006-01-02", date)
	if err != nil || parsed.Format("2006-01-02") != date {
		return fmt.Errorf("invalid due date %q, expected YYYY-MM-DD", date)
	}
	return nil
}

func ValidState(state string) bool { return state == "open" || state == "done" || state == "cancelled" }
func ValidPriority(priority string) bool {
	return priority == "high" || priority == "med" || priority == "low"
}

func NormalizeTags(tags []string) []string {
	out := make([]string, 0, len(tags))
	seen := map[string]bool{}
	for _, tag := range tags {
		tag = strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(tag), "#"))
		if tag != "" && !seen[tag] {
			out = append(out, tag)
			seen[tag] = true
		}
	}
	return out
}

func invalidFrontmatter(err error) error { return fmt.Errorf("invalid YAML frontmatter: %w", err) }

// ParseDocument parses task frontmatter and leaves the body byte-for-byte intact.
func ParseDocument(text string) (Task, error) {
	text = strings.TrimPrefix(text, "\ufeff")
	first, rest, found := strings.Cut(text, "\n")
	if !found {
		rest = ""
	}
	if strings.TrimRightFunc(first, unicode.IsSpace) != "---" {
		return Task{}, ErrMissingFrontmatter
	}
	var yamlText, body string
	closed := false
	for offset := 0; offset < len(rest); {
		end := strings.IndexByte(rest[offset:], '\n')
		if end < 0 {
			end = len(rest)
		} else {
			end += offset + 1
		}
		line := rest[offset:end]
		if strings.TrimRightFunc(line, unicode.IsSpace) == "---" {
			yamlText, body, closed = rest[:offset], rest[end:], true
			break
		}
		offset = end
	}
	if !closed {
		return Task{}, ErrUnterminatedFrontmatter
	}
	var doc yaml.Node
	if err := yaml.Unmarshal([]byte(yamlText), &doc); err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	if len(doc.Content) == 0 || doc.Content[0].Kind != yaml.MappingNode {
		return Task{}, ErrForeignFrontmatter
	}
	budget := 1_000_000
	mapping, err := expandYAMLAliases(doc.Content[0], map[*yaml.Node]bool{}, &budget)
	if err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	normalizeRustScalars(mapping)
	if err := checkDuplicateKeys(mapping); err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	fields := map[string]*yaml.Node{}
	for i := 0; i+1 < len(mapping.Content); i += 2 {
		fields[mapping.Content[i].Value] = mapping.Content[i+1]
	}
	if fields["id"] == nil {
		return Task{}, ErrForeignFrontmatter
	}
	scalar := func(name string, required bool) (string, error) {
		n := fields[name]
		if n == nil {
			if required {
				return "", fmt.Errorf("missing field `%s`", name)
			}
			return "", nil
		}
		if n.Kind != yaml.ScalarNode {
			return "", fmt.Errorf("%s: expected a string", name)
		}
		return n.Value, nil
	}
	id, err := scalar("id", true)
	if err == nil {
		err = ValidateID(id)
	}
	if err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	title, err := scalar("title", true)
	if err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	state, err := scalar("state", true)
	if err != nil {
		return Task{}, invalidFrontmatter(err)
	}
	if !ValidState(state) {
		return Task{}, invalidFrontmatter(fmt.Errorf("state: unknown variant %q, expected one of `open`, `done`, `cancelled`", state))
	}
	task := NewTask(id, title)
	task.State, task.Body = state, body
	for _, name := range []string{"parent", "due", "priority"} {
		n := fields[name]
		if n == nil || n.Tag == "!!null" {
			continue
		}
		value, err := scalar(name, false)
		if err != nil {
			return Task{}, invalidFrontmatter(err)
		}
		switch name {
		case "parent":
			if value != "" {
				if err := ValidateID(value); err != nil {
					return Task{}, invalidFrontmatter(err)
				}
				task.Parent = &value
			}
		case "due":
			if err := ValidateDate(value); err != nil {
				return Task{}, invalidFrontmatter(err)
			}
			task.Due = &value
		case "priority":
			if !ValidPriority(value) {
				return Task{}, invalidFrontmatter(fmt.Errorf("priority: unknown variant %q, expected one of `high`, `med`, `low`", value))
			}
			task.Priority = &value
		}
	}
	if n := fields["tags"]; n != nil {
		if n.Kind != yaml.SequenceNode {
			return Task{}, invalidFrontmatter(errors.New("tags: expected a sequence"))
		}
		for _, item := range n.Content {
			if item.Kind != yaml.ScalarNode {
				return Task{}, invalidFrontmatter(errors.New("tags: expected a string"))
			}
			task.Tags = append(task.Tags, item.Value)
		}
		task.Tags = NormalizeTags(task.Tags)
	}
	if n := fields["rank"]; n != nil && n.Tag != "!!null" {
		rank, err := parseRank(n)
		if err != nil {
			return Task{}, invalidFrontmatter(err)
		}
		task.Rank = &rank
	}
	for key, value := range fields {
		switch key {
		case "id", "title", "state", "parent", "tags", "due", "priority", "rank":
		default:
			task.Extra[key] = value
		}
	}
	return task, nil
}

// rustStringNumber identifies numeric-looking YAML scalars that serde_yaml
// resolves as strings rather than YAML 1.1 numbers.
func rustStringNumber(value string) bool {
	if strings.Contains(value, "_") {
		return true
	}
	digits := strings.TrimPrefix(strings.TrimPrefix(value, "+"), "-")
	if strings.HasPrefix(digits, "0X") || strings.HasPrefix(digits, "0O") || strings.HasPrefix(digits, "0B") {
		return true
	}
	if len(digits) < 2 || digits[0] != '0' {
		return false
	}
	for _, r := range digits[1:] {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

func normalizeRustScalars(node *yaml.Node) {
	if node.Kind == yaml.ScalarNode && node.Style&(yaml.TaggedStyle|yaml.SingleQuotedStyle|yaml.DoubleQuotedStyle|yaml.LiteralStyle|yaml.FoldedStyle) == 0 {
		if node.Tag == "!!timestamp" || (node.Tag == "!!int" || node.Tag == "!!float") && rustStringNumber(node.Value) {
			node.Tag = "!!str"
		}
	}
	for _, child := range node.Content {
		normalizeRustScalars(child)
	}
}

func parseRank(node *yaml.Node) (int32, error) {
	invalid := errors.New("rank: expected a 32-bit integer")
	if node.Kind != yaml.ScalarNode || node.Tag != "!!int" || node.Style&(yaml.SingleQuotedStyle|yaml.DoubleQuotedStyle|yaml.LiteralStyle|yaml.FoldedStyle) != 0 {
		return 0, invalid
	}
	digits := node.Value
	sign := ""
	if strings.HasPrefix(digits, "+") || strings.HasPrefix(digits, "-") {
		sign, digits = digits[:1], digits[1:]
	}
	base := 10
	switch {
	case strings.HasPrefix(digits, "0x"):
		base, digits = 16, digits[2:]
	case strings.HasPrefix(digits, "0o"):
		base, digits = 8, digits[2:]
	case strings.HasPrefix(digits, "0b"):
		base, digits = 2, digits[2:]
	case len(digits) > 1 && digits[0] == '0':
		return 0, invalid
	}
	if strings.Contains(digits, "_") {
		return 0, invalid
	}
	value, err := strconv.ParseInt(sign+digits, base, 32)
	if err != nil {
		return 0, invalid
	}
	return int32(value), nil
}

func expandYAMLAliases(node *yaml.Node, visiting map[*yaml.Node]bool, budget *int) (*yaml.Node, error) {
	if node == nil || visiting[node] {
		return nil, errors.New("recursive or invalid YAML alias")
	}
	if *budget <= 0 {
		return nil, errors.New("YAML alias expansion exceeds limit")
	}
	(*budget)--
	visiting[node] = true
	defer delete(visiting, node)
	if node.Kind == yaml.AliasNode {
		return expandYAMLAliases(node.Alias, visiting, budget)
	}
	copy := *node
	copy.Anchor = ""
	copy.Alias = nil
	copy.Content = make([]*yaml.Node, len(node.Content))
	for i, child := range node.Content {
		var err error
		copy.Content[i], err = expandYAMLAliases(child, visiting, budget)
		if err != nil {
			return nil, err
		}
	}
	return &copy, nil
}

func checkDuplicateKeys(n *yaml.Node) error {
	if n.Kind == yaml.MappingNode {
		seen := map[string]bool{}
		for i := 0; i+1 < len(n.Content); i += 2 {
			key := n.Content[i].Value
			if seen[key] {
				return fmt.Errorf("duplicate entry with key %q", key)
			}
			seen[key] = true
		}
	}
	for _, child := range n.Content {
		if err := checkDuplicateKeys(child); err != nil {
			return err
		}
	}
	return nil
}

func stringNode(value string) *yaml.Node {
	n := &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: value}
	// serde_yaml uses single quotes for ambiguous scalar strings, and treats
	// dates and timestamps as strings without requiring quotes.
	if value == "" || strings.HasPrefix(value, "`") {
		n.Style = yaml.SingleQuotedStyle
		return n
	}
	var parsed yaml.Node
	if yaml.Unmarshal([]byte(value), &parsed) == nil && len(parsed.Content) == 1 {
		scalar := parsed.Content[0]
		if scalar.Kind == yaml.ScalarNode && scalar.Value == value {
			switch scalar.Tag {
			case "!!timestamp":
				n.Tag = scalar.Tag
			case "!!int", "!!float":
				if strings.Contains(value, "_") || strings.Contains(value, "0X") || strings.Contains(value, "0O") || strings.Contains(value, "0B") {
					n.Tag = scalar.Tag
				} else {
					n.Style = yaml.SingleQuotedStyle
				}
			case "!!null", "!!bool":
				n.Style = yaml.SingleQuotedStyle
			}
		}
	}
	return n
}

// blockStyleExtra mirrors serde_yaml's block-style serialization of collections.
func blockStyleExtra(node *yaml.Node) *yaml.Node {
	copy := *node
	copy.Style &^= yaml.FlowStyle
	copy.HeadComment, copy.LineComment, copy.FootComment = "", "", ""
	if node.Kind == yaml.ScalarNode && node.Tag == "!!str" {
		plain := stringNode(node.Value)
		copy.Tag, copy.Style = plain.Tag, plain.Style
	}
	copy.Content = make([]*yaml.Node, len(node.Content))
	for i, child := range node.Content {
		copy.Content[i] = blockStyleExtra(child)
	}
	return &copy
}

// Document serializes known fields in the Rust writer's order and extras sorted.
func (task Task) Document() (string, error) {
	mapping := &yaml.Node{Kind: yaml.MappingNode, Tag: "!!map"}
	add := func(key string, value *yaml.Node) {
		mapping.Content = append(mapping.Content, stringNode(key), value)
	}
	declared := task.DeclaredID
	if declared == "" {
		declared = task.ID
	}
	add("id", stringNode(declared))
	add("title", stringNode(task.Title))
	add("state", stringNode(task.State))
	if task.Parent != nil {
		add("parent", stringNode(*task.Parent))
	}
	if len(task.Tags) > 0 {
		seq := &yaml.Node{Kind: yaml.SequenceNode, Tag: "!!seq"}
		for _, tag := range task.Tags {
			seq.Content = append(seq.Content, stringNode(tag))
		}
		add("tags", seq)
	}
	if task.Due != nil {
		add("due", &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!timestamp", Value: *task.Due})
	}
	if task.Priority != nil {
		add("priority", stringNode(*task.Priority))
	}
	if task.Rank != nil {
		add("rank", &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!int", Value: strconv.FormatInt(int64(*task.Rank), 10)})
	}
	keys := make([]string, 0, len(task.Extra))
	for key := range task.Extra {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		add(key, blockStyleExtra(task.Extra[key]))
	}
	marker, nonBMP := maskNonBMP(mapping)
	var out bytes.Buffer
	encoder := yaml.NewEncoder(&out)
	encoder.SetIndent(2)
	encoder.CompactSeqIndent()
	if err := encoder.Encode(mapping); err != nil {
		return "", err
	}
	if err := encoder.Close(); err != nil {
		return "", err
	}
	encoded, err := restoreNonBMP(out.String(), marker, nonBMP)
	if err != nil {
		return "", err
	}
	return "---\n" + strings.TrimRight(encoded, "\r\n") + "\n---\n" + task.Body, nil
}
