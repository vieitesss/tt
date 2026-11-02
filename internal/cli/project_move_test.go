package cli

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestProjectRegistrationResolutionAndRemoval(t *testing.T) {
	s := sandbox(t)
	current := s.json("project", "current")
	if current["registered"] != false || current["path"] != s.project || current["slug"] != nil || current["nested"] != nil {
		t.Fatalf("unregistered current: %#v", current)
	}
	id := s.add("Auto-register")
	store := filepath.Join(s.data, "tt", "project")
	if _, err := os.Stat(filepath.Join(store, id+".md")); err != nil {
		t.Fatalf("task not stored centrally: %v", err)
	}
	sub := filepath.Join(s.project, "sub")
	if err := os.Mkdir(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	current = s.json("--path", sub, "project", "current")
	if current["registered"] != true || current["path"] != s.project || current["slug"] != "project" || current["nested"] != sub {
		t.Fatalf("current nested: %#v", current)
	}
	// No explicit DIR is idempotent for the nearest registered ancestor.
	added := s.json("--path", sub, "project", "add")
	if added["path"] != s.project || added["slug"] != "project" {
		t.Fatalf("implicit project add should use ancestor: %#v", added)
	}
	added = s.json("project", "add", sub)
	if added["path"] != sub || added["slug"] != "sub" {
		t.Fatalf("explicit nested add: %#v", added)
	}
	current = s.json("--path", sub, "project", "current")
	if current["path"] != sub || current["nested"] != nil {
		t.Fatalf("nearest explicit project: %#v", current)
	}
	projects := s.json("project", "list")["projects"].([]any)
	if len(projects) != 2 || projects[0].(map[string]any)["never_ask_nested"] != false {
		t.Fatalf("projects: %#v", projects)
	}
	removed := s.json("project", "remove", "sub")["removed"].(map[string]any)
	if removed["slug"] != "sub" {
		t.Fatalf("remove by slug: %#v", removed)
	}
	removed = s.json("project", "remove", s.project)["removed"].(map[string]any)
	if removed["path"] != s.project {
		t.Fatalf("remove by path: %#v", removed)
	}
	if _, err := os.Stat(filepath.Join(store, id+".md")); err != nil {
		t.Fatalf("remove deleted stored task: %v", err)
	}
}

func TestLegacyVaultWarningAndMissingRegisteredDirectory(t *testing.T) {
	s := sandbox(t)
	legacy := filepath.Join(s.root, "legacy-vault")
	if err := os.WriteFile(s.config, []byte("vault = \""+legacy+"\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := s.run("", "--json", "add", "Legacy ignored")
	var task map[string]any
	if code != 0 || json.Unmarshal([]byte(stdout), &task) != nil || !strings.Contains(stderr, "warning: ") || !strings.Contains(stderr, "deprecated") {
		t.Fatalf("legacy config: %d %q %q", code, stdout, stderr)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "project", task["id"].(string)+".md")); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(s.config)
	if err != nil || !strings.Contains(string(data), "vault = ") {
		t.Fatalf("legacy config key lost: %q %v", data, err)
	}
	if err := os.Remove(s.project); err != nil {
		t.Fatal(err)
	}
	code, stdout, _ = s.run("", "--json", "list")
	if code != 1 || !strings.Contains(stdout, "registered project directory is missing") {
		t.Fatalf("missing registered project: %d %q", code, stdout)
	}
}

func TestMoveWithinAndAcrossProjects(t *testing.T) {
	s := sandbox(t)
	destination := filepath.Join(s.root, "target")
	if err := os.Mkdir(destination, 0o755); err != nil {
		t.Fatal(err)
	}
	s.json("project", "add")
	s.json("project", "add", destination)
	parent := s.add("Parent")
	moving := s.add("Moving", "--body", "moving body")
	child := s.add("Child", "--parent", moving)
	within := s.json("move", moving, "--parent", parent)
	if within["target_project"].(map[string]any)["slug"] != "project" || len(within["tasks"].([]any)) != 2 || s.json("show", moving)["parent"] != parent {
		t.Fatalf("move within store: %#v", within)
	}
	code, stdout, _ := s.run("", "--json", "move", moving, "--parent", child)
	if code != 1 || !strings.Contains(stdout, "cannot parent itself or a descendant") || s.json("show", moving)["parent"] != parent {
		t.Fatalf("cycle rejection: %d %q", code, stdout)
	}
	within = s.json("move", moving, "--to-project", s.project, "--root")
	if len(within["tasks"].([]any)) != 2 || s.json("show", moving)["parent"] != nil {
		t.Fatalf("move to current project: %#v", within)
	}
	destinationParent := s.json("--path", destination, "add", "Destination")["id"].(string)
	targetStore := filepath.Join(s.data, "tt", "target")
	collision := filepath.Join(targetStore, moving+".md")
	if err := os.WriteFile(collision, []byte("---\nid: "+moving+"\ntitle: Collision\nstate: open\n---\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, _ = s.run("", "--json", "move", moving, "--to-project", "target")
	if code != 1 || !strings.Contains(stdout, "already exists") {
		t.Fatalf("collision: %d %q", code, stdout)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "project", moving+".md")); err != nil {
		t.Fatalf("collision removed source: %v", err)
	}
	if err := os.Remove(collision); err != nil {
		t.Fatal(err)
	}
	moved := s.json("move", moving, "--to-project", destination, "--parent", destinationParent)
	if moved["target_project"].(map[string]any)["path"] != destination || len(moved["tasks"].([]any)) != 2 {
		t.Fatalf("cross-project move: %#v", moved)
	}
	shown := s.json("--path", destination, "show", moving)
	if shown["parent"] != destinationParent || shown["rank"] != nil || shown["body"] != "moving body" {
		t.Fatalf("moved task: %#v", shown)
	}
	if shownChild := s.json("--path", destination, "show", child); shownChild["parent"] != moving {
		t.Fatalf("child: %#v", shownChild)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "project", moving+".md")); !os.IsNotExist(err) {
		t.Fatalf("source still present: %v", err)
	}
}

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v: %s", args, err, out)
	}
}

func TestWorktreesShareStoreAndMapExactProjectPaths(t *testing.T) {
	s := sandbox(t)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	git(t, s.project, "init", "-q")
	if err := os.WriteFile(filepath.Join(s.project, "tracked.txt"), []byte("fixture\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, s.project, "add", "tracked.txt")
	git(t, s.project, "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "fixture")
	linked := filepath.Join(s.root, "linked")
	git(t, s.project, "worktree", "add", "-qb", "linked-test", linked)
	for _, dir := range []string{filepath.Join(s.project, "nested", "below"), filepath.Join(linked, "nested", "below"), filepath.Join(linked, "not-registered")} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	// Invalid mapped paths must not even trigger a pending config migration.
	legacy := filepath.Join(s.root, "legacy", "tt", "config.toml")
	if err := os.MkdirAll(filepath.Dir(legacy), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(legacy, []byte("capture_target = 'legacy'\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TT_CONFIG", "")
	missingMapped := filepath.Join(s.project, "not-registered")
	code, stdout, stderr := s.run("", "--json", "project", "add", filepath.Join(linked, "not-registered"))
	if code != 1 || stderr != "" || !strings.Contains(stdout, "directory does not exist in the main checkout: "+missingMapped) {
		t.Fatalf("missing mapped directory before migration: %d %q %q", code, stdout, stderr)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt")); !os.IsNotExist(err) {
		t.Fatalf("invalid registration wrote data or migrated config: %v", err)
	}
	t.Setenv("TT_CONFIG", s.config)
	// An unregistered worktree add registers the common main root.
	added := s.json("--path", linked, "add", "From worktree")
	id := added["id"].(string)
	if tasks := s.json("list", "--flat")["tasks"].([]any); len(tasks) != 1 || tasks[0].(map[string]any)["id"] != id {
		t.Fatalf("worktrees did not share the main store: %#v", tasks)
	}
	projects := s.json("project", "list")["projects"].([]any)
	if len(projects) != 1 || projects[0].(map[string]any)["path"] != s.project {
		t.Fatalf("worktree registered wrong project: %#v", projects)
	}
	beforeConfig, err := os.ReadFile(s.config)
	if err != nil {
		t.Fatal(err)
	}
	missingMain := filepath.Join(s.project, "not-registered")
	code, stdout, stderr = s.run("", "--json", "project", "add", filepath.Join(linked, "not-registered"))
	if code != 1 || stderr != "" || !strings.Contains(stdout, "directory does not exist in the main checkout: "+missingMain) {
		t.Fatalf("missing mapped directory: %d %q %q", code, stdout, stderr)
	}
	afterConfig, err := os.ReadFile(s.config)
	if err != nil || string(afterConfig) != string(beforeConfig) {
		t.Fatalf("missing mapped directory changed config: %q %v", afterConfig, err)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "not-registered")); !os.IsNotExist(err) {
		t.Fatalf("missing mapped directory created a store: %v", err)
	}
	mainNested := filepath.Join(s.project, "nested")
	linkedNested := filepath.Join(linked, "nested")
	explicit := s.json("project", "add", linkedNested)
	if explicit["path"] != mainNested || explicit["slug"] != "nested" {
		t.Fatalf("explicit linked subdirectory must map to its main equivalent: %#v", explicit)
	}
	implicit := s.json("--path", filepath.Join(linkedNested, "below"), "project", "add")
	if implicit["path"] != mainNested || implicit["slug"] != "nested" {
		t.Fatalf("implicit nested add must be idempotent: %#v", implicit)
	}
	current := s.json("--path", filepath.Join(linkedNested, "below"), "project", "current")
	if current["path"] != mainNested || current["nested"] != filepath.Join(mainNested, "below") {
		t.Fatalf("mapped nested current: %#v", current)
	}
	// Exact path matching happens after linked-to-main mapping, not ancestor resolution.
	code, stdout, _ = s.run("", "--json", "move", id, "--to-project", filepath.Join(linked, "not-registered"))
	if code != 1 || !strings.Contains(stdout, "not a registered project") {
		t.Fatalf("unregistered linked path: %d %q", code, stdout)
	}
	code, stdout, _ = s.run("", "--json", "project", "remove", filepath.Join(linked, "not-registered"))
	if code != 1 || !strings.Contains(stdout, "not a registered project") {
		t.Fatalf("remove unregistered linked path: %d %q", code, stdout)
	}
	moved := s.json("move", id, "--to-project", linkedNested)
	if moved["target_project"].(map[string]any)["path"] != mainNested {
		t.Fatalf("mapped destination: %#v", moved)
	}
	if shown := s.json("--path", linkedNested, "show", id); shown["id"] != id {
		t.Fatalf("worktree destination task: %#v", shown)
	}
	removed := s.json("project", "remove", linkedNested)["removed"].(map[string]any)
	if removed["path"] != mainNested {
		t.Fatalf("mapped project remove: %#v", removed)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "nested", id+".md")); err != nil {
		t.Fatalf("remove deleted nested Store: %v", err)
	}
}
