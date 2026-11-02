package cli

import (
	"bufio"
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/project"
)

func TestCharacterDeviceWithoutTTYIsNoninteractive(t *testing.T) {
	s := sandbox(t)
	null, err := os.OpenFile(os.DevNull, os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer null.Close()
	if isTerminal(null) {
		t.Fatal("the null device is not a terminal")
	}
	var stderr bytes.Buffer
	if code := Run([]string{"--path", s.project, "add", "Task"}, null, null, &stderr); code != 0 {
		t.Fatalf("noninteractive add failed: %d %q", code, stderr.String())
	}
	if !strings.Contains(s.json("project", "list")["projects"].([]any)[0].(map[string]any)["slug"].(string), "project") {
		t.Fatal("noninteractive add did not register the project")
	}
}

func TestInteractiveRegistrationAndNestedPreferences(t *testing.T) {
	s := sandbox(t)
	config, err := project.Load()
	if err != nil {
		t.Fatal(err)
	}
	var stdout, stderr bytes.Buffer
	input := strings.NewReader("no\n")
	a := &app{in: input, out: &stdout, err: &stderr, path: s.project, reader: bufio.NewReader(input), terminal: true}
	if _, err := a.openVault(config, true); err == nil || err.Error() != unregistered {
		t.Fatalf("declined registration: %v", err)
	}
	if len(config.Projects) != 0 || !strings.Contains(stderr.String(), "register ") {
		t.Fatalf("decline mutated config or omitted prompt: %+v %q", config.Projects, stderr.String())
	}
	input = strings.NewReader("yes\n")
	a.in, a.reader = input, bufio.NewReader(input)
	if _, err := a.openVault(config, true); err != nil || len(config.Projects) != 1 {
		t.Fatalf("confirmed registration: %+v %v", config.Projects, err)
	}
	nested := filepath.Join(s.project, "nested")
	if err := os.Mkdir(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	a.path = nested
	input = strings.NewReader("!\n")
	a.in, a.reader = input, bufio.NewReader(input)
	if _, err := a.openVault(config, true); err != nil {
		t.Fatal(err)
	}
	reloaded, err := project.Load()
	if err != nil || len(reloaded.Projects) != 1 || !reloaded.Projects[0].NeverAskNested {
		t.Fatalf("never-ask preference: %+v %v", reloaded, err)
	}
	// Never-ask is effective on later invocations, including EOF on stdin.
	input = strings.NewReader("")
	a.in, a.reader = input, bufio.NewReader(input)
	stderr.Reset()
	if _, err := a.openVault(reloaded, true); err != nil || stderr.Len() != 0 {
		t.Fatalf("never-ask should suppress prompt: %q %v", stderr.String(), err)
	}
}

func TestNestedPromptDoesNotRegisterMissingMainCheckoutDirectory(t *testing.T) {
	s := sandbox(t)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	git(t, s.project, "init", "-q")
	if err := os.WriteFile(filepath.Join(s.project, "tracked.txt"), []byte("fixture\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, s.project, "add", "tracked.txt")
	git(t, s.project, "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "fixture")
	linked := filepath.Join(s.root, "linked")
	git(t, s.project, "worktree", "add", "-qb", "nested-prompt-test", linked)
	if err := os.Mkdir(filepath.Join(linked, "new"), 0o755); err != nil {
		t.Fatal(err)
	}
	config, err := project.Load()
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := config.RegisterAndSave(s.project); err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(s.config)
	if err != nil {
		t.Fatal(err)
	}
	var stdout, stderr bytes.Buffer
	input := strings.NewReader("yes\n")
	a := &app{in: input, out: &stdout, err: &stderr, path: filepath.Join(linked, "new"), reader: bufio.NewReader(input), terminal: true}
	if _, err := a.openVault(config, true); err == nil || !strings.Contains(err.Error(), filepath.Join(s.project, "new")) {
		t.Fatalf("expected missing mapped directory error: %v", err)
	}
	after, err := os.ReadFile(s.config)
	if err != nil || string(after) != string(before) || len(config.Projects) != 1 {
		t.Fatalf("invalid nested registration changed config: %q %+v %v", after, config.Projects, err)
	}
	if _, err := os.Stat(filepath.Join(s.data, "tt", "new")); !os.IsNotExist(err) {
		t.Fatalf("invalid nested registration created store: %v", err)
	}
}

func TestInteractiveNestedRegistrationCanCreateSeparateStore(t *testing.T) {
	s := sandbox(t)
	config, err := project.Load()
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := config.RegisterAndSave(s.project); err != nil {
		t.Fatal(err)
	}
	nested := filepath.Join(s.project, "nested")
	if err := os.Mkdir(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	var stdout, stderr bytes.Buffer
	input := strings.NewReader("yes\n")
	a := &app{in: input, out: &stdout, err: &stderr, path: nested, reader: bufio.NewReader(input), terminal: true}
	v, err := a.openVault(config, true)
	if err != nil || len(config.Projects) != 2 || filepath.Base(v.Root()) != "nested" {
		t.Fatalf("nested registration: %+v %v", config.Projects, err)
	}
}
