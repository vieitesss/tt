package vault_test

import (
	"reflect"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestApplyReorderMaterializesRanksOnlyWhenNeeded(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"a.md": "---\nid: a\ntitle: A\nstate: open\n---\n",
		"b.md": "---\nid: b\ntitle: B\nstate: open\n---\n",
		"c.md": "---\nid: c\ntitle: C\nstate: open\n---\n",
		"p.md": "---\nid: p\ntitle: P\nstate: open\n---\n",
		"x.md": "---\nid: x\ntitle: Only child\nstate: open\nparent: p\n---\n",
	}
	writeTasks(t, dir, fixtures)
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, err := v.Revision()
	if err != nil {
		t.Fatal(err)
	}
	a, b, c, p, x := "a", "b", "c", "p", "x"
	nodes := []vault.DesiredTask{{ID: &a, Title: "A"}, {ID: &b, Title: "B"}, {ID: &c, Title: "C"}, {ID: &p, Title: "P", Children: []vault.DesiredTask{{ID: &x, Title: "Only child"}}}}
	plan, err := v.Apply(vault.ApplyInput{Revision: revision, Tasks: nodes}, false)
	if err != nil || plan.Reorders != 0 {
		t.Fatalf("no-op apply: %+v %v", plan, err)
	}
	for name, original := range fixtures {
		if data := readTask(t, dir, name); data != original {
			t.Fatalf("no-op rewrote %s: %q", name, data)
		}
	}
	nodes[0], nodes[1] = nodes[1], nodes[0]
	plan, err = v.Apply(vault.ApplyInput{Revision: plan.Revision, Tasks: nodes}, false)
	if err != nil || plan.Reorders != 4 {
		t.Fatalf("reorder: %+v %v", plan, err)
	}
	if got := v.Roots(); !reflect.DeepEqual(got, []string{"b", "a", "c", "p"}) {
		t.Fatalf("reordered roots: %v", got)
	}
	for i, id := range v.Roots() {
		task, _ := v.Get(id)
		if task.Rank == nil || *task.Rank != int32(i) {
			t.Fatalf("rank %s: %+v", id, task.Rank)
		}
	}
	if data := readTask(t, dir, "x.md"); data != fixtures["x.md"] {
		t.Fatalf("unaffected child rewritten: %q", data)
	}
	reloaded, err := vault.Open(dir)
	if err != nil || !reflect.DeepEqual(reloaded.Roots(), v.Roots()) {
		t.Fatalf("rank did not persist: %v %v", reloaded.Roots(), err)
	}
}
