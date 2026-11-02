package project_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/vieitesss/tt/internal/project"
)

func runGit(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v: %s", args, err, output)
	}
}

func makeWorktrees(t *testing.T) (string, string, string, string) {
	t.Helper()
	tmp := t.TempDir()
	main := filepath.Join(tmp, "main")
	if err := os.Mkdir(main, 0o755); err != nil {
		t.Fatal(err)
	}
	runGit(t, main, "init", "-q")
	runGit(t, main, "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "--allow-empty", "-m", "initial")
	linked := filepath.Join(tmp, "linked")
	runGit(t, main, "worktree", "add", "-q", "--detach", linked)
	bare := filepath.Join(tmp, "bare.git")
	runGit(t, tmp, "clone", "-q", "--bare", main, bare)
	bareLinked := filepath.Join(tmp, "bare-linked")
	runGit(t, bare, "worktree", "add", "-q", "--detach", bareLinked)
	for _, root := range []string{main, linked, bareLinked} {
		if err := os.MkdirAll(filepath.Join(root, "sub"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	return main, linked, bare, bareLinked
}

func TestWorktreeResolutionAndStoreIdentity(t *testing.T) {
	main, linked, bare, bareLinked := makeWorktrees(t)
	t.Setenv("XDG_DATA_HOME", t.TempDir())
	t.Setenv("HOME", t.TempDir())
	config, err := project.LoadFrom(filepath.Join(t.TempDir(), "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	mainProject, _, err := config.RegisterAndSave(main)
	if err != nil {
		t.Fatal(err)
	}
	for _, start := range []string{main, filepath.Join(main, "sub"), linked, filepath.Join(linked, "sub")} {
		got := project.Resolve(config, start)
		if got.Project == nil || got.Project.Slug != mainProject.Slug {
			t.Errorf("resolve %s: %+v", start, got)
		}
	}
	got := project.Resolve(config, filepath.Join(linked, "sub"))
	if mapped := project.MapWorktreePath(filepath.Join(linked, "sub")); mapped != project.Normalize(filepath.Join(main, "sub")) {
		t.Errorf("mapped worktree subdirectory: %s", mapped)
	}
	if got.Nested == nil || *got.Nested != project.Normalize(filepath.Join(main, "sub")) {
		t.Errorf("linked subdirectory nested: %+v", got)
	}
	for _, start := range []string{bareLinked, filepath.Join(bareLinked, "sub")} {
		got := project.Resolve(config, start)
		if got.Project != nil || got.Path != project.Normalize(bare) {
			t.Errorf("unregistered bare-linked %s: %+v", start, got)
		}
	}
	bareProject, _, err := config.RegisterAndSave(bare)
	if err != nil {
		t.Fatal(err)
	}
	if got := project.Resolve(config, filepath.Join(bareLinked, "sub")); got.Project == nil || got.Project.Slug != bareProject.Slug {
		t.Errorf("bare linked worktree: %+v", got)
	}
	worktreeProject, _, err := config.RegisterAndSave(filepath.Join(linked, "sub"))
	if err != nil {
		t.Fatal(err)
	}
	if got := project.Resolve(config, filepath.Join(linked, "sub")); got.Project == nil || got.Project.Slug != worktreeProject.Slug {
		t.Errorf("explicit linked registration should win: %+v", got)
	}
	nonGit := filepath.Join(t.TempDir(), "plain")
	if err := os.Mkdir(nonGit, 0o755); err != nil {
		t.Fatal(err)
	}
	if got := project.Resolve(config, nonGit); got.Project != nil || got.Path != project.Normalize(nonGit) {
		t.Errorf("non-git: %+v", got)
	}
}
