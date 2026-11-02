package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func snapshotMarkdown(t *testing.T, dir string) map[string]string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	out := map[string]string{}
	for _, entry := range entries {
		if !entry.IsDir() && filepath.Ext(entry.Name()) == ".md" {
			data, err := os.ReadFile(filepath.Join(dir, entry.Name()))
			if err != nil {
				t.Fatal(err)
			}
			out[entry.Name()] = string(data)
		}
	}
	return out
}

func applyInput(t *testing.T, revision string, tasks any) string {
	t.Helper()
	encoded, err := json.Marshal(map[string]any{"revision": revision, "tasks": tasks})
	if err != nil {
		t.Fatal(err)
	}
	return string(encoded)
}

type editOnWarning struct {
	path string
	data string
}

func (w editOnWarning) Write(p []byte) (int, error) {
	if err := os.WriteFile(w.path, []byte(w.data), 0o644); err != nil {
		return 0, err
	}
	return len(p), nil
}

func TestListRevisionMatchesReturnedTasksDuringExternalEdit(t *testing.T) {
	for _, flat := range []bool{false, true} {
		t.Run(map[bool]string{false: "tree", true: "flat"}[flat], func(t *testing.T) {
			s := sandbox(t)
			store := filepath.Join(s.root, "store")
			if err := os.Mkdir(store, 0o755); err != nil {
				t.Fatal(err)
			}
			path := filepath.Join(store, "a.md")
			original := "---\nid: a\ntitle: Original\nstate: open\n---\n"
			external := strings.Replace(original, "Original", "External edit", 1)
			if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(store, "broken.md"), []byte("---\nid: broken\ntitle: [bad\n---\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			args := []string{"--vault", store, "--json", "list"}
			if flat {
				args = append(args, "--flat")
			}
			var stdout bytes.Buffer
			code := Run(args, strings.NewReader(""), &stdout, editOnWarning{path, external})
			var listed struct {
				Revision string `json:"revision"`
				Tasks    []struct {
					Title string `json:"title"`
				} `json:"tasks"`
			}
			if code != 0 || json.Unmarshal(stdout.Bytes(), &listed) != nil || len(listed.Tasks) != 1 || listed.Tasks[0].Title != "Original" {
				t.Fatalf("list: exit %d %q", code, stdout.String())
			}
			input := applyInput(t, listed.Revision, []any{map[string]any{"id": "a", "title": listed.Tasks[0].Title}})
			code, output, _ := s.run(input, "--vault", store, "--json", "apply")
			if code != 1 || !strings.Contains(output, "task list changed on disk; reload") {
				t.Fatalf("stale list allowed apply: exit %d %q", code, output)
			}
			if data, err := os.ReadFile(path); err != nil || string(data) != external {
				t.Fatalf("external edit lost: %q %v", data, err)
			}
		})
	}
}

func TestRevisionDistinguishesDifferentStoresWithSameConcatenatedBytes(t *testing.T) {
	s := sandbox(t)
	store := filepath.Join(s.root, "store")
	if err := os.Mkdir(store, 0o755); err != nil {
		t.Fatal(err)
	}
	a := "---\nid: a\ntitle: A\nstate: open\n---\n"
	b := "---\nid: b\ntitle: B\nstate: open\n---\nImportant body\n"
	pathA, pathB := filepath.Join(store, "a.md"), filepath.Join(store, "b.md")
	if err := os.WriteFile(pathA, []byte(a+"b"+b), 0o644); err != nil {
		t.Fatal(err)
	}
	original := s.json("--vault", store, "list")
	if err := os.WriteFile(pathA, []byte(a), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte(b), 0o644); err != nil {
		t.Fatal(err)
	}
	changed := s.json("--vault", store, "list")
	if original["revision"] == changed["revision"] {
		t.Fatalf("different Stores have the same revision: %v", original["revision"])
	}
	input := applyInput(t, original["revision"].(string), []any{map[string]any{"id": "a", "title": "A"}})
	code, output, _ := s.run(input, "--vault", store, "--json", "apply")
	if code != 1 || !strings.Contains(output, "task list changed on disk; reload") {
		t.Fatalf("stale apply: exit %d %q", code, output)
	}
	if data, err := os.ReadFile(pathB); err != nil || string(data) != b {
		t.Fatalf("new task lost: %q %v", data, err)
	}
}

func TestApplyCLIReportsRealPartialFilesystemFailure(t *testing.T) {
	s := sandbox(t)
	store := filepath.Join(s.root, "read-only-store")
	if err := os.Mkdir(store, 0o755); err != nil {
		t.Fatal(err)
	}
	original := "---\nid: a\ntitle: A\nstate: open\n---\n"
	if err := os.WriteFile(filepath.Join(store, "a.md"), []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}
	revision := s.json("--vault", store, "list")["revision"].(string)
	input := applyInput(t, revision, []any{map[string]any{"id": "a", "title": "A", "state": "done"}})
	if err := os.Chmod(store, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(store, 0o755) })
	probe := filepath.Join(store, "probe")
	if err := os.WriteFile(probe, nil, 0o644); err == nil {
		_ = os.Remove(probe)
		t.Skip("filesystem permits writes despite directory permissions")
	}
	code, stdout, stderr := s.run(input, "--vault", store, "--json", "apply")
	var response map[string]any
	if code != 1 || stderr != "" || json.Unmarshal([]byte(stdout), &response) != nil || response["error"] == nil {
		t.Fatalf("JSON partial failure: %d %q %q", code, stdout, stderr)
	}
	applied, ok := response["applied"].(map[string]any)
	if !ok || applied["revision"] != revision || len(applied["updates"].([]any)) != 0 {
		t.Fatalf("JSON applied progress: %#v", response)
	}
	code, stdout, stderr = s.run(input, "--vault", store, "apply")
	if code != 1 || stdout != "" || !strings.HasPrefix(stderr, "error: ") || !strings.Contains(stderr, "; applied: ") {
		t.Fatalf("human partial failure: %d %q %q", code, stdout, stderr)
	}
	if data, err := os.ReadFile(filepath.Join(store, "a.md")); err != nil || string(data) != original {
		t.Fatalf("failed write changed task: %q %v", data, err)
	}
}

func TestApplyCLIPlansExecutesAndRejectsStaleInput(t *testing.T) {
	s := sandbox(t)
	root := s.add("Root", "--body", "keep this body\n")
	child := s.add("Child", "--parent", root)
	grandchild := s.add("Grandchild", "--parent", child, "--body", "important\n")
	keeper := s.add("Keeper", "--body", "[["+root+".md|Root]]")
	list := s.json("list")
	revision := list["revision"].(string)
	desired := []any{
		map[string]any{"id": keeper, "title": "Keeper", "children": []any{}},
		map[string]any{"id": root, "title": "Renamed", "children": []any{map[string]any{"title": "New child"}}},
	}
	input := applyInput(t, revision, desired)
	store := filepath.Join(s.data, "tt", "project")
	before := snapshotMarkdown(t, store)
	code, stdout, stderr := s.run(input, "--json", "apply", "--dry-run")
	var plan map[string]any
	if code != 0 || stderr != "" || json.Unmarshal([]byte(stdout), &plan) != nil || plan["revision"] != revision || plan["dry_run"] != true {
		t.Fatalf("dry run: %d %q %q", code, stdout, stderr)
	}
	if len(plan["adds"].([]any)) != 1 || len(plan["deletes"].([]any)) != 2 || !reflect.DeepEqual(before, snapshotMarkdown(t, store)) {
		t.Fatalf("dry run plan or write: %#v", plan)
	}
	deletes := map[string]bool{}
	for _, deletion := range plan["deletes"].([]any) {
		d := deletion.(map[string]any)
		deletes[d["id"].(string)] = d["has_body"].(bool)
	}
	if _, ok := deletes[child]; !ok || !deletes[grandchild] {
		t.Fatalf("descendant delete plan: %#v", deletes)
	}
	code, stdout, stderr = s.run(input, "--json", "apply")
	var applied map[string]any
	if code != 0 || stderr != "" || json.Unmarshal([]byte(stdout), &applied) != nil || applied["dry_run"] != false || applied["revision"] == revision {
		t.Fatalf("apply: %d %q %q", code, stdout, stderr)
	}
	created := applied["adds"].([]any)[0].(map[string]any)
	if created["parent"] != root || created["title"] != "New child" {
		t.Fatalf("new child: %#v", created)
	}
	if shown := s.json("show", root); shown["body"] != "keep this body\n" || shown["title"] != "Renamed" {
		t.Fatalf("renamed task: %#v", shown)
	}
	if shown := s.json("show", keeper); shown["body"] != "[["+root+".md|Renamed]]" {
		t.Fatalf("mirror not synchronized: %#v", shown)
	}
	for _, deleted := range []string{child, grandchild} {
		if _, err := os.Stat(filepath.Join(store, deleted+".md")); !os.IsNotExist(err) {
			t.Fatalf("descendant %s remains: %v", deleted, err)
		}
	}
	after := snapshotMarkdown(t, store)
	code, stdout, stderr = s.run(input, "--json", "apply")
	var failure map[string]any
	if code != 1 || stderr != "" || json.Unmarshal([]byte(stdout), &failure) != nil || failure["error"] != "task list changed on disk; reload" || !reflect.DeepEqual(after, snapshotMarkdown(t, store)) {
		t.Fatalf("stale apply: %d %q %q", code, stdout, stderr)
	}
	if current := s.json("list")["revision"]; current != applied["revision"] {
		t.Fatalf("applied revision %v, current %v", applied["revision"], current)
	}
}

func TestApplyCLIRejectsInvalidInputBeforeWriting(t *testing.T) {
	s := sandbox(t)
	id := s.add("Existing")
	revision := s.json("list")["revision"].(string)
	store := filepath.Join(s.data, "tt", "project")
	before := snapshotMarkdown(t, store)
	cases := []string{
		"{}", "null", `{"revision":"` + revision + `","tasks":null}`, `{"revision":"` + revision + `","tasks":[]} {}`,
		applyInput(t, revision, []any{map[string]any{"id": id, "title": ""}}),
		applyInput(t, revision, []any{map[string]any{"id": id, "title": "Existing"}, map[string]any{"id": id, "title": "Again"}}),
		applyInput(t, revision, []any{map[string]any{"id": "missing", "title": "Missing"}}),
		applyInput(t, revision, []any{map[string]any{"id": id, "title": "Existing", "state": "unknown"}}),
		applyInput(t, revision, []any{map[string]any{"id": id, "title": "Existing", "due": "2026-02-30"}}),
	}
	for _, input := range cases {
		code, stdout, stderr := s.run(input, "--json", "apply")
		var value map[string]any
		if code != 1 || stderr != "" || json.Unmarshal([]byte(stdout), &value) != nil || value["error"] == nil || !reflect.DeepEqual(before, snapshotMarkdown(t, store)) {
			t.Errorf("invalid %s: %d %q %q", input, code, stdout, stderr)
		}
	}
	code, stdout, stderr := s.run(applyInput(t, "stale", []any{}), "apply")
	if code != 1 || stdout != "" || !strings.Contains(stderr, "error: task list changed on disk; reload") {
		t.Fatalf("human error: %d %q %q", code, stdout, stderr)
	}
}
