package cli

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestTaskLifecycleTreeFiltersAndHumanOutput(t *testing.T) {
	s := sandbox(t)
	parent := s.add("Parent")
	child := s.add("Child", "--parent", parent, "--tag", "#work/admin", "--tag", "work/admin", "--due", "2000-01-01", "--priority", "high")
	other := s.add("Other", "--tag", "workshop")

	tree := s.json("list")
	roots := tree["tasks"].([]any)
	if len(roots) != 2 || roots[0].(map[string]any)["id"] != parent {
		t.Fatalf("tree roots: %#v", roots)
	}
	nested := roots[0].(map[string]any)["children"].([]any)
	if len(nested) != 1 || nested[0].(map[string]any)["id"] != child || tree["revision"] == "" {
		t.Fatalf("nested tree: %#v", tree)
	}
	filtered := s.json("list", "--flat", "--tag", "work")["tasks"].([]any)
	if len(filtered) != 1 || filtered[0].(map[string]any)["id"] != child {
		t.Fatalf("tag hierarchy filter: %#v", filtered)
	}
	promoted := s.json("list", "--tag", "work")["tasks"].([]any)
	if len(promoted) != 1 || promoted[0].(map[string]any)["id"] != child {
		t.Fatalf("filtered child should be promoted: %#v", promoted)
	}
	for _, tc := range []struct{ command, state string }{{"done", "done"}, {"reopen", "open"}, {"cancel", "cancelled"}} {
		if result := s.json(tc.command, child); result["id"] != child || result["state"] != tc.state {
			t.Fatalf("%s: %#v", tc.command, result)
		}
	}
	filtered = s.json("list", "--flat", "--state", "cancelled", "--due-today")["tasks"].([]any)
	if len(filtered) != 1 || filtered[0].(map[string]any)["id"] != child {
		t.Fatalf("combined filters: %#v", filtered)
	}
	shown := s.json("show", child)
	if shown["parent"] != parent || shown["priority"] != "high" || shown["due"] != "2000-01-01" || shown["path"] != filepath.Join(s.data, "tt", "project", child+".md") {
		t.Fatalf("show: %#v", shown)
	}
	if tags := shown["tags"].([]any); len(tags) != 1 || tags[0] != "work/admin" {
		t.Fatalf("tags: %#v", tags)
	}
	if result := s.json("edit", child, "--title", "Renamed"); result["title"] != "Renamed" || result["state"] != "cancelled" {
		t.Fatalf("edit: %#v", result)
	}
	code, human, stderr := s.run("", "list")
	if code != 0 || stderr != "" || !strings.Contains(human, "Renamed") || !strings.Contains(human, other) || strings.HasPrefix(strings.TrimSpace(human), "{") {
		t.Fatalf("human list: exit %d %q %q", code, human, stderr)
	}
	for _, id := range []string{"missing000", "bad/id"} {
		code, stdout, stderr := s.run("", "--json", "show", id)
		var value map[string]string
		if code != 1 || stderr != "" || json.Unmarshal([]byte(stdout), &value) != nil || !strings.Contains(value["error"], id) {
			t.Fatalf("show %q error: %d %q %q", id, code, stdout, stderr)
		}
	}
}

func TestLinksBacklinksAndRenameMirrors(t *testing.T) {
	s := sandbox(t)
	target := s.add("Old title")
	body := "ñ [[dir/" + target + ".md|Old title]] [[" + target + "|context]] `[[" + target + "|Old title]]`\n```\n[[" + target + "|Old title]]\n```\n"
	source := s.add("Source", "--body", body)
	shown := s.json("show", source)
	links := shown["links"].([]any)
	backlinks := s.json("show", target)["backlinks"].([]any)
	if len(links) != 1 || links[0] != target || len(backlinks) != 1 || backlinks[0] != source {
		t.Fatalf("links=%#v backlinks=%#v", links, backlinks)
	}
	if edited := s.json("edit", target, "--title", "New title"); edited["title"] != "New title" || edited["mirror_files_updated"] != nil {
		t.Fatalf("edit output: %#v", edited)
	}
	bodyAfter := s.json("show", source)["body"].(string)
	if !strings.Contains(bodyAfter, "[[dir/"+target+".md|New title]]") || !strings.Contains(bodyAfter, "[["+target+"|context]]") || strings.Count(bodyAfter, "|Old title]]") != 2 {
		t.Fatalf("mirror rewrite: %q", bodyAfter)
	}
}

func TestAddRejectsInvalidUTF8FromStdinWithoutChangingSiblings(t *testing.T) {
	s := sandbox(t)
	store := filepath.Join(s.root, "vault")
	t.Setenv("TT_VAULT", store)
	_ = s.add("Existing")
	before := snapshotMarkdown(t, store)
	code, stdout, stderr := s.run(string([]byte{0xff}), "--json", "add", "Invalid", "--body", "-")
	var response map[string]any
	if code != 1 || stderr != "" || json.Unmarshal([]byte(stdout), &response) != nil || !strings.Contains(fmt.Sprint(response["error"]), "UTF-8") {
		t.Fatalf("invalid body: exit %d %q %q", code, stdout, stderr)
	}
	if after := snapshotMarkdown(t, store); !reflect.DeepEqual(before, after) {
		t.Fatalf("invalid body changed the Store: before %#v after %#v", before, after)
	}
}

func TestBodyStdinVaultEnvCaptureAndEditor(t *testing.T) {
	s := sandbox(t)
	v := filepath.Join(s.root, "vault")
	t.Setenv("TT_VAULT", v)
	parent := s.add("Capture parent")
	if err := os.WriteFile(s.config, []byte("capture_target = \""+parent+"\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := s.run("hello from stdin\n", "--json", "add", "--title", "Captured", "--body", "-")
	var result map[string]any
	if code != 0 || stderr != "" || json.Unmarshal([]byte(stdout), &result) != nil || result["parent"] != parent {
		t.Fatalf("stdin/capture: %d %q %q", code, stdout, stderr)
	}
	id := result["id"].(string)
	if result["path"] != filepath.Join(v, id+".md") || s.json("show", id)["body"] != "hello from stdin\n" {
		t.Fatalf("wrong store/body: %#v", result)
	}
	script := filepath.Join(s.root, "editor.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nfor path in \"$@\"; do :; done\nprintf '\\nfrom the editor\\n' >> \"$path\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("EDITOR", script+" --flag")
	if edited := s.json("edit", id); edited["id"] != id {
		t.Fatalf("editor result: %#v", edited)
	}
	if body := s.json("show", id)["body"].(string); !strings.Contains(body, "from the editor") {
		t.Fatalf("editor body: %q", body)
	}
	bin := filepath.Join(s.root, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bin, "vi"), []byte("#!/bin/sh\nprintf '\\nfallback editor\\n' >> \"$1\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
	t.Setenv("EDITOR", "  ")
	_ = s.json("edit", id)
	if body := s.json("show", id)["body"].(string); !strings.Contains(body, "fallback editor") {
		t.Fatalf("fallback editor did not run: %q", body)
	}
}

func TestUnrankedAndMalformedFiles(t *testing.T) {
	s := sandbox(t)
	v := filepath.Join(s.root, "vault")
	t.Setenv("TT_VAULT", v)
	if err := os.Mkdir(v, 0o755); err != nil {
		t.Fatal(err)
	}
	files := map[string]string{
		"unranked.md": "---\nid: unranked\ntitle: Unranked\nstate: open\n---\n",
		"foreign.md":  "---\ntitle: Foreign\n---\n",
		"broken.md":   "---\nid: broken\ntitle: [bad\n---\n",
		"mismatch.md": "---\nid: declared\ntitle: Mismatch\nstate: open\n---\n",
	}
	for name, text := range files {
		if err := os.WriteFile(filepath.Join(v, name), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	code, stdout, stderr := s.run("", "--json", "list", "--flat")
	var result map[string]any
	if code != 0 || json.Unmarshal([]byte(stdout), &result) != nil || !strings.Contains(stderr, "warning: ") || !strings.Contains(stderr, "malformed") || !strings.Contains(stderr, "id mismatch") || strings.Contains(stderr, "foreign") {
		t.Fatalf("issues: %d %q %q", code, stdout, stderr)
	}
	tasks := result["tasks"].([]any)
	if len(tasks) != 2 {
		t.Fatalf("expected two tasks: %#v", tasks)
	}
	for _, item := range tasks {
		task := item.(map[string]any)
		if task["id"] == "unranked" && task["rank"] != nil {
			t.Fatalf("unranked: %#v", task)
		}
	}
	_ = s.json("done", "mismatch")
	data, err := os.ReadFile(filepath.Join(v, "mismatch.md"))
	if err != nil || !strings.Contains(string(data), "id: declared\n") {
		t.Fatalf("declared id not preserved: %q %v", data, err)
	}
	for _, name := range []string{"foreign.md", "broken.md"} {
		data, err := os.ReadFile(filepath.Join(v, name))
		if err != nil || string(data) != files[name] {
			t.Fatalf("%s rewritten: %q %v", name, data, err)
		}
	}
}
