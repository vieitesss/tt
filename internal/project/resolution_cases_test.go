package project_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/vieitesss/tt/internal/project"
)

func TestStructuralResolutionAndStartPrecedence(t *testing.T) {
	dir := t.TempDir()
	root := filepath.Join(dir, "project")
	nested := filepath.Join(root, "nested")
	child := filepath.Join(nested, "child")
	sibling := filepath.Join(dir, "project-other")
	for _, path := range []string{child, sibling} {
		if err := os.MkdirAll(path, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	config := &project.Config{Projects: []project.Project{
		{Path: filepath.Join(root, "."), Slug: "root", NeverAskNested: true},
		{Path: nested, Slug: "nested"},
	}}
	// Force Git discovery to fail: structural resolution remains usable.
	t.Setenv("PATH", "")
	for _, tc := range []struct {
		start  string
		slug   string
		nested string
	}{
		{root, "root", ""},
		{nested, "nested", ""},
		{child, "nested", project.Normalize(child)},
		{sibling, "", ""},
	} {
		got := project.Resolve(config, tc.start)
		if tc.slug == "" {
			if got.Project != nil || got.Path != project.Normalize(tc.start) {
				t.Errorf("unregistered %s: %+v", tc.start, got)
			}
			continue
		}
		if got.Project == nil || got.Project.Slug != tc.slug || (got.Nested == nil) != (tc.nested == "") || got.Nested != nil && *got.Nested != tc.nested {
			t.Errorf("resolve %s: %+v", tc.start, got)
		}
	}
	t.Setenv("TT_PATH", nested)
	if got, err := project.StartDir(root); err != nil || got != root {
		t.Fatalf("explicit path: %q %v", got, err)
	}
	if got, err := project.StartDir(""); err != nil || got != nested {
		t.Fatalf("TT_PATH: %q %v", got, err)
	}
	t.Setenv("TT_PATH", "")
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if got, err := project.StartDir(""); err != nil || got != cwd {
		t.Fatalf("cwd: %q %v", got, err)
	}
}
