package vault_test

import (
	"errors"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestForeignNotesDuplicatesAndRankValidation(t *testing.T) {
	for _, document := range []string{"---\n---\n", "---\n- a\n- b\n---\n", "---\ntitle: note\n---\n"} {
		if _, err := vault.ParseDocument(document); !errors.Is(err, vault.ErrForeignFrontmatter) {
			t.Errorf("foreign note %q: %v", document, err)
		}
	}
	for _, document := range []string{
		"---\nid: a\ntitle: A\ntitle: B\nstate: open\n---\n",
		"---\nid: a\ntitle: A\nstate: open\nrank: 2147483648\n---\n",
	} {
		if _, err := vault.ParseDocument(document); err == nil {
			t.Errorf("expected invalid frontmatter: %q", document)
		}
	}
}

func TestExtrasUseRustBlockStyle(t *testing.T) {
	task, err := vault.ParseDocument("---\nid: a\ntitle: A\nstate: open\ncustom:\n  nested: [1, 2]\n---\n")
	if err != nil {
		t.Fatal(err)
	}
	written, err := task.Document()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(written, "custom:\n  nested:\n  - 1\n  - 2\n") {
		t.Fatalf("expected block-style extra values, got:\n%s", written)
	}
}

func TestTaskDocumentRoundTripAndScalarCompatibility(t *testing.T) {
	document := "---\nid: 123\ntitle: 456\nstate: open\nparent: ~\ntags: ['#work', work, 'home']\ndue: 2026-11-03\nrank: 4\ncustom: 42\n---\nbody\n"
	task, err := vault.ParseDocument(document)
	if err != nil {
		t.Fatal(err)
	}
	if task.ID != "123" || task.Title != "456" || task.Parent != nil || len(task.Tags) != 2 {
		t.Fatalf("unexpected task: %+v", task)
	}
	written, err := task.Document()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(written, "due: 2026-11-03\n") || !strings.Contains(written, "custom: 42\n") {
		t.Fatalf("unexpected document:\n%s", written)
	}
	reparsed, err := vault.ParseDocument(written)
	if err != nil || reparsed.Title != task.Title || reparsed.Body != "body\n" {
		t.Fatalf("round trip: %+v, %v", reparsed, err)
	}
}
