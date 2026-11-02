package vault_test

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func writeTasks(t *testing.T, dir string, files map[string]string) {
	t.Helper()
	for name, text := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
	}
}

func readTask(t *testing.T, dir, name string) string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func TestAddRejectsInvalidUTF8WithoutChangingTask(t *testing.T) {
	dir := t.TempDir()
	original := "---\nid: a\ntitle: A\nstate: open\n---\nBody\n"
	writeTasks(t, dir, map[string]string{"a.md": original})
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Add(vault.AddInput{Title: "Invalid", Body: string([]byte{0xff})}); err == nil || !strings.Contains(err.Error(), "UTF-8") {
		t.Fatalf("invalid body accepted: %v", err)
	}
	if got := readTask(t, dir, "a.md"); got != original {
		t.Fatalf("invalid body changed file: %q", got)
	}
	if task, ok := store.Get("a"); !ok || task.Body != "Body\n" {
		t.Fatalf("invalid body changed cached task: %+v", task)
	}
	if entries, err := os.ReadDir(dir); err != nil || len(entries) != 1 {
		t.Fatalf("invalid body created a task: %v %v", entries, err)
	}
}

func TestApplySwapsParentsDeletesOnlyAbsentAndLeavesIssuesUnwritten(t *testing.T) {
	dir := t.TempDir()
	files := map[string]string{
		"a.md":    "---\nid: a\ntitle: A\nstate: open\n---\n",
		"b.md":    "---\nid: b\ntitle: B\nstate: open\nparent: a\n---\n",
		"c.md":    "---\nid: c\ntitle: C\nstate: open\nparent: a\n---\n",
		"d.md":    "---\nid: d\ntitle: D\nstate: open\nparent: b\n---\nImportant\n",
		"dang.md": "---\nid: dang\ntitle: Dangling\nstate: open\nparent: missing\n---\nbody",
		"x.md":    "---\nid: x\ntitle: X\nstate: open\nparent: y\n---\n",
		"y.md":    "---\nid: y\ntitle: Y\nstate: open\nparent: x\n---\n",
		"z.md":    "---\nid: z\ntitle: Z\nstate: open\n# retain comment\n---\n",
		"bad.md":  "---\nid: bad\ntitle: Broken\ntitle: duplicate\nstate: open\n---\n",
	}
	writeTasks(t, dir, files)
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, _ := store.Revision()
	a, b, c, dang, x, y, z := "a", "b", "c", "dang", "x", "y", "z"
	input := vault.ApplyInput{Revision: revision, Tasks: []vault.DesiredTask{
		{ID: &b, Title: "B", Children: []vault.DesiredTask{{ID: &a, Title: "A"}}},
		{ID: &c, Title: "C"}, {ID: &dang, Title: "Dangling"},
		{ID: &x, Title: "X", Children: []vault.DesiredTask{{ID: &y, Title: "Y"}}},
		{ID: &z, Title: "Z"},
	}}
	plan, err := store.Apply(input, false)
	if err != nil {
		t.Fatal(err)
	}
	if len(plan.Moves) != 3 || len(plan.Deletes) != 1 || plan.Deletes[0].ID != "d" || !plan.Deletes[0].HasBody || plan.Reorders != 0 {
		t.Fatalf("unexpected plan: %+v", plan)
	}
	if _, err := os.Stat(filepath.Join(dir, "d.md")); !os.IsNotExist(err) {
		t.Fatalf("deleted descendant still exists: %v", err)
	}
	if got := store.Children("b"); !reflect.DeepEqual(got, []string{"a"}) {
		t.Fatalf("swap: %v", got)
	}
	if _, ok := store.Get("c"); !ok || store.Parent("c") != nil {
		t.Fatal("moved-out child did not survive at root")
	}
	for _, name := range []string{"dang.md", "x.md", "y.md", "z.md", "bad.md"} {
		if got := readTask(t, dir, name); got != files[name] {
			t.Errorf("unexpected write to %s: %q", name, got)
		}
	}
}

func TestApplyOnlyWritesRanksThatChange(t *testing.T) {
	dir := t.TempDir()
	files := map[string]string{
		"a.md": "---\nid: a\ntitle: A\nstate: open\nrank: 0\n# untouched\n---\n",
		"b.md": "---\nid: b\ntitle: B\nstate: open\nrank: 2\n---\n",
		"c.md": "---\nid: c\ntitle: C\nstate: open\nrank: 1\n---\n",
	}
	writeTasks(t, dir, files)
	store, _ := vault.Open(dir)
	revision, _ := store.Revision()
	a, b, c := "a", "b", "c"
	input := vault.ApplyInput{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: "A"}, {ID: &b, Title: "B"}, {ID: &c, Title: "C"}}}
	plan, err := store.Apply(input, false)
	if err != nil || plan.Reorders != 2 {
		t.Fatalf("reorder: %+v %v", plan, err)
	}
	if got := readTask(t, dir, "a.md"); got != files["a.md"] {
		t.Fatalf("rank already correct was rewritten: %q", got)
	}
	if got := store.Roots(); !reflect.DeepEqual(got, []string{"a", "b", "c"}) {
		t.Fatalf("order: %v", got)
	}
	input.Revision = plan.Revision
	second, err := store.Apply(input, false)
	if err != nil || second.Reorders != 0 {
		t.Fatalf("idempotent reorder: %+v %v", second, err)
	}
}

func TestApplyRejectsStaleRevisionAndInvalidInputWithoutWrites(t *testing.T) {
	dir := t.TempDir()
	original := "---\nid: a\ntitle: A\nstate: open\n---\nbody\n"
	writeTasks(t, dir, map[string]string{"a.md": original})
	store, _ := vault.Open(dir)
	revision, _ := store.Revision()
	a, missing, invalidState, invalidPriority, invalidDue := "a", "missing", "invalid", "urgent", "2026-02-30"
	cases := []vault.ApplyInput{
		{Revision: "stale", Tasks: []vault.DesiredTask{{ID: &a, Title: "A"}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: " "}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: "A"}, {ID: &a, Title: "A"}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &missing, Title: "Missing"}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: "A", State: &invalidState}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: "A", Priority: &invalidPriority}}},
		{Revision: revision, Tasks: []vault.DesiredTask{{ID: &a, Title: "A", Due: &invalidDue}}},
	}
	for _, input := range cases {
		if _, err := store.Apply(input, false); err == nil {
			t.Errorf("expected rejection: %+v", input)
		}
		if got := readTask(t, dir, "a.md"); got != original {
			t.Errorf("invalid input wrote file: %q", got)
		}
	}
}

func TestApplyDryRunAndExecuteRenameAndAdd(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"one.md": "---\nid: one\ntitle: Old\nstate: open\n---\nbody\n",
		"two.md": "---\nid: two\ntitle: Link\nstate: open\n---\n[[one.md|Old]]\n",
	}
	for name, data := range fixtures {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, err := store.Revision()
	if err != nil {
		t.Fatal(err)
	}
	one, two := "one", "two"
	input := vault.ApplyInput{Revision: revision, Tasks: []vault.DesiredTask{
		{ID: &two, Title: "Link", Children: []vault.DesiredTask{}},
		{ID: &one, Title: "New", Children: []vault.DesiredTask{{Title: "Added"}}},
	}}
	plan, err := store.Apply(input, true)
	if err != nil {
		t.Fatal(err)
	}
	if !plan.DryRun || len(plan.Adds) != 1 || len(plan.Updates) == 0 || plan.Revision != revision {
		t.Fatalf("dry plan: %+v", plan)
	}
	for name, want := range fixtures {
		got, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil || string(got) != want {
			t.Fatalf("dry run changed %s: %q, %v", name, got, err)
		}
	}
	result, err := store.Apply(input, false)
	if err != nil {
		t.Fatal(err)
	}
	if result.DryRun || len(result.Adds) != 1 || result.Revision == revision {
		t.Fatalf("result: %+v", result)
	}
	child := result.Adds[0]
	if child.Parent == nil || *child.Parent != one {
		t.Fatalf("new child parent: %+v", child)
	}
	link, _ := store.Get(two)
	if !strings.Contains(link.Body, "[[one.md|New]]") {
		t.Fatalf("mirror not synchronized: %q", link.Body)
	}
	renamed, _ := store.Get(one)
	if renamed.Body != "body\n" {
		t.Fatalf("body changed: %q", renamed.Body)
	}
}
