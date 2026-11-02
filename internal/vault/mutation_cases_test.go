package vault_test

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestMutatorValidationNeverWritesMalformedTasks(t *testing.T) {
	dir := t.TempDir()
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	task, err := v.Add(vault.AddInput{Title: "Original"})
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, task.ID+".md")
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	invalidDue, invalidPriority, invalidParent := "2026-02-30", "urgent", "bad/id"
	for _, input := range []vault.AddInput{
		{Title: "Bad due", Due: &invalidDue},
		{Title: "Bad priority", Priority: &invalidPriority},
		{Title: "Bad parent", Parent: &invalidParent},
	} {
		if _, err := v.Add(input); err == nil {
			t.Errorf("invalid add accepted: %+v", input)
		}
	}
	if after, err := os.ReadFile(path); err != nil || !reflect.DeepEqual(before, after) {
		t.Fatalf("invalid input rewrote original: %q %v", after, err)
	}
	if entries, err := os.ReadDir(dir); err != nil || len(entries) != 1 {
		t.Fatalf("invalid input created files: %v %v", entries, err)
	}
}

func TestAddFieldsAndApplyMetadataRoundTrip(t *testing.T) {
	dir := t.TempDir()
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	first, err := v.Add(vault.AddInput{Title: "Zebra"})
	if err != nil {
		t.Fatal(err)
	}
	due, priority := "2026-01-02", "high"
	body := "See [[" + first.ID + "]]\n"
	second, err := v.Add(vault.AddInput{Title: "Apple", Due: &due, Priority: &priority, Tags: []string{"#work", " work ", "home", ""}, Body: body})
	if err != nil {
		t.Fatal(err)
	}
	if got := v.Roots(); !reflect.DeepEqual(got, []string{first.ID, second.ID}) {
		t.Fatalf("append order: %v", got)
	}
	if got := v.Backlinks(first.ID); !reflect.DeepEqual(got, []string{second.ID}) {
		t.Fatalf("body index: %v", got)
	}
	if _, err := v.SetState(second.ID, "done"); err != nil {
		t.Fatal(err)
	}
	reopened, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	task, ok := reopened.Get(second.ID)
	if !ok || task.State != "done" || task.Due == nil || *task.Due != due || task.Priority == nil || *task.Priority != priority || task.Body != body || !reflect.DeepEqual(task.Tags, []string{"work", "home"}) {
		t.Fatalf("fields after reopen: %+v", task)
	}
	revision, err := reopened.Revision()
	if err != nil {
		t.Fatal(err)
	}
	done, nextDue, nextPriority := "done", "2026-03-04", "low"
	plan, err := reopened.Apply(vault.ApplyInput{Revision: revision, Tasks: []vault.DesiredTask{
		{ID: &first.ID, Title: first.Title},
		{ID: &second.ID, Title: second.Title, State: &done, Due: &nextDue, Priority: &nextPriority, Tags: []string{"#new", " new ", "home"}},
	}}, false)
	if err != nil {
		t.Fatal(err)
	}
	reopened, err = vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	task, ok = reopened.Get(second.ID)
	if !ok || task.Due == nil || *task.Due != nextDue || task.Priority == nil || *task.Priority != nextPriority || !reflect.DeepEqual(task.Tags, []string{"new", "home"}) || task.Body != body {
		t.Fatalf("updated metadata or preserved body: %+v", task)
	}
	_, err = reopened.Apply(vault.ApplyInput{Revision: plan.Revision, Tasks: []vault.DesiredTask{
		{ID: &first.ID, Title: first.Title},
		{ID: &second.ID, Title: second.Title, State: &done},
	}}, false)
	if err != nil {
		t.Fatal(err)
	}
	reopened, err = vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	task, ok = reopened.Get(second.ID)
	if !ok || task.Due != nil || task.Priority != nil || len(task.Tags) != 0 || task.State != "done" || task.Body != body {
		t.Fatalf("cleared metadata or preserved body: %+v", task)
	}
	if got := reopened.Backlinks(first.ID); !reflect.DeepEqual(got, []string{second.ID}) {
		t.Fatalf("body index after apply: %v", got)
	}
	if _, err := reopened.SetState("missing", "done"); err == nil {
		t.Error("unknown task state mutation succeeded")
	}
	if _, _, err := reopened.SetTitle("missing", "X"); err == nil {
		t.Error("unknown task title mutation succeeded")
	}
}

func TestSetParentClosesRankGapAndPreservesExtras(t *testing.T) {
	dir := t.TempDir()
	writeTasks(t, dir, map[string]string{
		"a.md": "---\nid: a\ntitle: A\nstate: open\nrank: 0\ncustom: 42\n---\n",
		"b.md": "---\nid: b\ntitle: B\nstate: open\nrank: 1\n---\n",
		"c.md": "---\nid: c\ntitle: C\nstate: open\nrank: 2\n---\n",
		"p.md": "---\nid: p\ntitle: P\nstate: open\nrank: 3\n---\n",
	})
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	parent := "p"
	if _, err := v.SetParent("a", &parent); err != nil {
		t.Fatal(err)
	}
	if got := v.Roots(); !reflect.DeepEqual(got, []string{"b", "c", "p"}) {
		t.Fatalf("remaining roots: %v", got)
	}
	for i, id := range v.Roots() {
		task, _ := v.Get(id)
		if task.Rank == nil || *task.Rank != int32(i) {
			t.Fatalf("rank gap for %s: %+v", id, task.Rank)
		}
	}
	if data, err := os.ReadFile(filepath.Join(dir, "a.md")); err != nil || !strings.Contains(string(data), "custom: 42") {
		t.Fatalf("extras lost: %q %v", data, err)
	}
	if _, err := v.SetParent("a", nil); err != nil {
		t.Fatal(err)
	}
	if task, _ := v.Get("a"); task.Parent != nil {
		t.Fatalf("root parent: %+v", task)
	}
	missing := "missing"
	if _, err := v.SetParent("a", &missing); err == nil {
		t.Fatal("unknown parent accepted")
	}
	if _, err := v.SetParent("missing", &parent); err == nil {
		t.Fatal("unknown task accepted")
	}
}
