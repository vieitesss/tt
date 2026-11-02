package vault

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	"go.yaml.in/yaml/v3"
)

func TestModelRequiredFieldsAndOptionalDefaults(t *testing.T) {
	task, err := ParseDocument("---\nid: abc1234567\ntitle: Minimal\nstate: open\n---\n")
	if err != nil || task.Parent != nil || len(task.Tags) != 0 || task.Due != nil || task.Priority != nil || task.Rank != nil || task.Body != "" || len(task.Extra) != 0 {
		t.Fatalf("defaults: %+v %v", task, err)
	}
	for _, tc := range []struct {
		name string
		doc  string
		err  error
	}{
		{"missing frontmatter", "id: a\ntitle: A\nstate: open\n", ErrMissingFrontmatter},
		{"empty", "", ErrMissingFrontmatter},
		{"unterminated", "---\nid: a\ntitle: A\nstate: open\n", ErrUnterminatedFrontmatter},
		{"foreign", "---\ntitle: T\nstate: open\n---\n", ErrForeignFrontmatter},
		{"foreign empty", "---\n---\n", ErrForeignFrontmatter},
		{"foreign sequence", "---\n- a\n- b\n---\n", ErrForeignFrontmatter},
		{"missing title", "---\nid: a\nstate: open\n---\n", nil},
		{"missing state", "---\nid: a\ntitle: A\n---\n", nil},
		{"invalid state", "---\nid: a\ntitle: A\nstate: almost\n---\n", nil},
		{"invalid priority", "---\nid: a\ntitle: A\nstate: open\npriority: urgent\n---\n", nil},
		{"invalid due", "---\nid: a\ntitle: A\nstate: open\ndue: 2026-02-30\n---\n", nil},
		{"invalid id", "---\nid: BAD\ntitle: A\nstate: open\n---\n", nil},
		{"invalid rank", "---\nid: a\ntitle: A\nstate: open\nrank: 2147483648\n---\n", nil},
		{"duplicate key", "---\nid: a\ntitle: A\ntitle: B\nstate: open\n---\n", nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, err := ParseDocument(tc.doc)
			if err == nil || tc.err != nil && !errors.Is(err, tc.err) {
				t.Fatalf("got %v, want %v", err, tc.err)
			}
		})
	}
}

func TestStringScalarsRetainNullAndNumericSpellings(t *testing.T) {
	for _, tc := range []struct{ yaml, title string }{
		{"null", "null"}, {"~", "~"}, {"", ""}, {"Null", "Null"}, {"true", "true"}, {"123", "123"},
	} {
		t.Run(tc.yaml, func(t *testing.T) {
			doc := "---\nid: a\ntitle: " + tc.yaml + "\nstate: open\ntags: [null, ~, '', 12, true]\n---\n"
			task, err := ParseDocument(doc)
			if err != nil || task.Title != tc.title || !reflect.DeepEqual(task.Tags, []string{"null", "~", "12", "true"}) {
				t.Fatalf("string scalars: %+v %v", task, err)
			}
			written, err := task.Document()
			if err != nil {
				t.Fatal(err)
			}
			roundTrip, err := ParseDocument(written)
			if err != nil || roundTrip.Title != task.Title || !reflect.DeepEqual(roundTrip.Tags, task.Tags) {
				t.Fatalf("string roundtrip: %q %+v %v", written, roundTrip, err)
			}
		})
	}
}

func TestNonBMPScalarsRemainLiteralAndRoundTrip(t *testing.T) {
	doc := "---\nid: a\ntitle: Launch 😀\nstate: open\ntags: [ready/🚀, smile/😃]\nextra: Globe 🌍\nnested: {🗺: [🧭, 'quoted: 🐱']}\n---\nbody 🌙\n"
	task, err := ParseDocument(doc)
	if err != nil {
		t.Fatal(err)
	}
	written, err := task.Document()
	if err != nil {
		t.Fatal(err)
	}
	for _, literal := range []string{"title: Launch 😀", "- ready/🚀", "- smile/😃", "extra: Globe 🌍", "🗺:", "- 🧭", "quoted: 🐱", "body 🌙"} {
		if !strings.Contains(written, literal) {
			t.Errorf("missing literal %q in %q", literal, written)
		}
	}
	if strings.Contains(written, `\U000`) {
		t.Fatalf("non-BMP character was escaped: %q", written)
	}
	reparsed, err := ParseDocument(written)
	if err != nil {
		t.Fatalf("reparse: %q %v", written, err)
	}
	if reparsed.Title != task.Title || !reflect.DeepEqual(reparsed.Tags, task.Tags) || reparsed.Body != task.Body || reparsed.Extra["extra"].Value != "Globe 🌍" {
		t.Fatalf("scalar roundtrip: %+v", reparsed)
	}
	nested := reparsed.Extra["nested"]
	if nested.Kind != yaml.MappingNode || len(nested.Content) != 2 || nested.Content[0].Value != "🗺" || len(nested.Content[1].Content) != 2 || nested.Content[1].Content[0].Value != "🧭" || nested.Content[1].Content[1].Value != "quoted: 🐱" {
		t.Fatalf("nested extra roundtrip: %+v", nested)
	}
	again, err := task.Document()
	if err != nil || again != written || task.Extra["extra"].Value != "Globe 🌍" {
		t.Fatalf("serialization mutated the source or changed output: %q %v", again, err)
	}
}

func TestNonBMPPlaceholderNeverCollidesWithInput(t *testing.T) {
	var allPrivate strings.Builder
	for r := '\uE000'; r <= '\uF8FF'; r++ {
		allPrivate.WriteRune(r)
	}
	allPrivate.WriteString("\uE000ttUnicode0\uE001 😀")
	for _, title := range []string{"\uE000 and \uE001 with 😀", allPrivate.String()} {
		task := NewTask("a", title)
		written, err := task.Document()
		if err != nil || !strings.Contains(written, "😀") {
			t.Fatalf("literal emoji missing: %v", err)
		}
		reparsed, err := ParseDocument(written)
		if err != nil || reparsed.Title != title {
			t.Fatalf("placeholder collision: %v", err)
		}
	}
}

func TestLiteralBackslashUnicodeSequenceRemainsText(t *testing.T) {
	for _, title := range []string{`Literal \U0001F600`, `Literal \U0001F600 with 😀`, `Quote: \U0001F600 with 😀`, "Tab:\t" + `\U0001F600 with 😀`} {
		task := NewTask("a", title)
		written, err := task.Document()
		if err != nil {
			t.Fatal(err)
		}
		if strings.Contains(title, "😀") && !strings.Contains(written, "😀") {
			t.Fatalf("emoji not literal: %q", written)
		}
		reparsed, err := ParseDocument(written)
		if err != nil || reparsed.Title != title {
			t.Errorf("literal backslash sequence changed: want %q, got %q; document %q, error %v", title, reparsed.Title, written, err)
		}
	}
}

func TestRustCompatibleStringScalarQuoting(t *testing.T) {
	for _, tc := range []struct{ title, line string }{
		{"", "title: ''"}, {"123", "title: '123'"}, {"000", "title: '000'"},
		{"1.0", "title: '1.0'"}, {"null", "title: 'null'"}, {"~", "title: '~'"},
		{"true", "title: 'true'"}, {"2024-01-02", "title: 2024-01-02"},
		{"2000-01-01T00:00:00Z", "title: 2000-01-01T00:00:00Z"},
		{"2024-01-02 # note", "title: '2024-01-02 # note'"},
	} {
		task := NewTask("a", tc.title)
		written, err := task.Document()
		if err != nil || !strings.Contains(written, "\n"+tc.line+"\n") {
			t.Errorf("title %q: %q %v", tc.title, written, err)
		}
		reparsed, err := ParseDocument(written)
		if err != nil || reparsed.Title != tc.title {
			t.Errorf("title %q roundtrip: %+v %v", tc.title, reparsed, err)
		}
	}
}

func TestExtraScalarsUseRustYAMLStringResolution(t *testing.T) {
	doc := "---\nid: a\ntitle: A\nstate: open\nleading: 010\nunderscores: 1_000\nuppercase: 0X10\ndate: 2024-01-02\ntext: \"hello\"\n---\n"
	task, err := ParseDocument(doc)
	if err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"leading", "underscores", "uppercase", "date", "text"} {
		if task.Extra[key].Tag != "!!str" {
			t.Errorf("extra %s should be a string, got %s", key, task.Extra[key].Tag)
		}
	}
	written, err := task.Document()
	if err != nil {
		t.Fatal(err)
	}
	for _, line := range []string{"leading: '010'", "underscores: 1_000", "uppercase: 0X10", "date: 2024-01-02", "text: hello"} {
		if !strings.Contains(written, "\n"+line+"\n") {
			t.Errorf("missing Rust-style scalar %q: %q", line, written)
		}
	}
	reparsed, err := ParseDocument(written)
	if err != nil {
		t.Fatal(err)
	}
	for key, extra := range task.Extra {
		if reparsed.Extra[key].Tag != extra.Tag || reparsed.Extra[key].Value != extra.Value || extra.Kind != yaml.ScalarNode {
			t.Errorf("extra %s changed: %+v -> %+v", key, extra, reparsed.Extra[key])
		}
	}
}

func TestRankUsesRustYAMLIntegerSyntaxAndI32Bounds(t *testing.T) {
	for _, tc := range []struct {
		raw   string
		valid bool
		want  int32
	}{
		{"0", true, 0}, {"+3", true, 3}, {"-2147483648", true, -2147483648}, {"2147483647", true, 2147483647},
		{"0x10", true, 16}, {"-0x10", true, -16}, {"+0o10", true, 8}, {"0b11", true, 3}, {"!!int 3", true, 3},
		{"010", false, 0}, {"+01", false, 0}, {"-01", false, 0}, {"1_000", false, 0}, {"0x1_0", false, 0},
		{"0X10", false, 0}, {"2147483648", false, 0}, {"-2147483649", false, 0}, {`!!int "3"`, false, 0},
	} {
		t.Run(tc.raw, func(t *testing.T) {
			doc := "---\nid: a\ntitle: A\nstate: open\nrank: " + tc.raw + "\n---\n"
			task, err := ParseDocument(doc)
			if tc.valid {
				if err != nil || task.Rank == nil || *task.Rank != tc.want {
					t.Fatalf("rank %q: %+v %v", tc.raw, task.Rank, err)
				}
			} else if err == nil {
				t.Fatalf("invalid rank %q accepted: %+v", tc.raw, task.Rank)
			}
		})
	}
}

func TestYAMLAliasesExpandAndRoundTripAsValues(t *testing.T) {
	doc := "---\nid: a\ntitle: &name 123\nstate: open\ntags: [*name]\nz-collection: &nested {x: 1}\na-copy: *nested\n---\n"
	task, err := ParseDocument(doc)
	if err != nil || task.Title != "123" || !reflect.DeepEqual(task.Tags, []string{"123"}) {
		t.Fatalf("aliases: %+v %v", task, err)
	}
	written, err := task.Document()
	if err != nil || strings.ContainsAny(written, "&*") {
		t.Fatalf("aliases were not expanded: %q %v", written, err)
	}
	if !strings.Contains(written, "a-copy:\n  x: 1\n") || !strings.Contains(written, "z-collection:\n  x: 1\n") {
		t.Fatalf("alias values lost: %q", written)
	}
	if _, err := ParseDocument(written); err != nil {
		t.Fatalf("expanded document is invalid: %q %v", written, err)
	}
	for _, recursive := range []string{
		"---\nid: a\ntitle: A\nstate: open\nextra: &a [*a]\n---\n",
		"---\nid: a\ntitle: &a [*a]\nstate: open\n---\n",
	} {
		if _, err := ParseDocument(recursive); err == nil {
			t.Fatal("recursive YAML alias accepted")
		}
	}
}

func TestFrontmatterDelimitersAcceptUnicodeTrailingWhitespace(t *testing.T) {
	doc := "---\u00a0\nid: a\ntitle: A\nstate: open\n---\u2003\nbody"
	task, err := ParseDocument(doc)
	if err != nil || task.ID != "a" || task.Body != "body" {
		t.Fatalf("Unicode whitespace around delimiters: %+v %v", task, err)
	}
}

func TestModelRoundTripsBodyBOMCRLFRankAndExtras(t *testing.T) {
	body := "first\r\n\r\n---\r\n\r\n```rust\r\nlet x = 1;\r\n```\r\nlast\r\n"
	doc := "\ufeff---\r\nid: abc1234567\r\ntitle: T\r\nstate: open\r\ntags: ['#work', ' work ', work, '']\r\ndue: 2026-01-02\r\npriority: high\r\nrank: 3\r\ncustom: 42\r\n---\r\n" + body
	task, err := ParseDocument(doc)
	if err != nil || task.Body != body || !reflect.DeepEqual(task.Tags, []string{"work"}) || task.Rank == nil || *task.Rank != 3 || task.Due == nil || *task.Due != "2026-01-02" {
		t.Fatalf("parse: %+v %v", task, err)
	}
	written, err := task.Document()
	if err != nil || !strings.HasSuffix(written, body) || !strings.Contains(written, "custom: 42\n") || !strings.Contains(written, "due: 2026-01-02\n") {
		t.Fatalf("write: %q %v", written, err)
	}
	reparsed, err := ParseDocument(written)
	if err != nil || reparsed.Body != body || !reflect.DeepEqual(reparsed.Tags, task.Tags) || *reparsed.Rank != *task.Rank {
		t.Fatalf("round trip: %+v %v", reparsed, err)
	}
	task.Rank = nil
	written, err = task.Document()
	if err != nil || strings.Contains(written, "rank:") {
		t.Fatalf("null rank should be omitted: %q %v", written, err)
	}
}

func TestIDGenerationValidationAndRetry(t *testing.T) {
	for _, invalid := range []string{"", "abc/def", "UPPER", strings.Repeat("a", 65), "ñ"} {
		if ValidateID(invalid) == nil {
			t.Errorf("invalid ID accepted: %q", invalid)
		}
	}
	if err := ValidateID("my_task-1"); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 100; i++ {
		id, err := generateID(func(string) bool { return false })
		if err != nil || len(id) != 10 || ValidateID(id) != nil || strings.ContainsAny(id, "-_") {
			t.Fatalf("generated ID %q: %v", id, err)
		}
	}
	attempts := 0
	id, err := generateID(func(string) bool { attempts++; return attempts <= 3 })
	if err != nil || attempts != 4 || ValidateID(id) != nil {
		t.Fatalf("retry: id=%q attempts=%d err=%v", id, attempts, err)
	}
}
