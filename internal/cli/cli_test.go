package cli_test

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

type sandbox struct {
	binary string
	root   string
	cwd    string
}

func newSandbox(t *testing.T) sandbox {
	t.Helper()
	root := t.TempDir()
	binary := filepath.Join(root, "tt")
	cmd := exec.Command("go", "build", "-o", binary, "../../cmd/tt")
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build tt: %v: %s", err, output)
	}
	cwd := filepath.Join(root, "project")
	if err := os.Mkdir(cwd, 0o755); err != nil {
		t.Fatal(err)
	}
	return sandbox{binary: binary, root: root, cwd: cwd}
}

func (s sandbox) run(t *testing.T, stdin string, args ...string) (map[string]any, string, int) {
	t.Helper()
	cmd := exec.Command(s.binary, args...)
	cmd.Dir = s.cwd
	cmd.Env = append(os.Environ(),
		"HOME="+s.root, "TT_CONFIG="+filepath.Join(s.root, "config.toml"), "XDG_DATA_HOME="+filepath.Join(s.root, "data"),
		"XDG_CONFIG_HOME="+filepath.Join(s.root, "xdg-config"), "TT_PATH=", "TT_VAULT=",
	)
	cmd.Stdin = strings.NewReader(stdin)
	var stdout, stderr strings.Builder
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	err := cmd.Run()
	code := 0
	if err != nil {
		if exit, ok := err.(*exec.ExitError); ok {
			code = exit.ExitCode()
		} else {
			t.Fatalf("run: %v", err)
		}
	}
	var value map[string]any
	if err := json.Unmarshal([]byte(stdout.String()), &value); err != nil {
		t.Fatalf("invalid JSON %q (stderr=%q, exit=%d): %v", stdout.String(), stderr.String(), code, err)
	}
	return value, stderr.String(), code
}

func TestAddAndShowReturnAbsolutePathsForRelativeVault(t *testing.T) {
	s := newSandbox(t)
	cwd, err := filepath.EvalSymlinks(s.cwd)
	if err != nil {
		t.Fatal(err)
	}
	added, stderr, code := s.run(t, "", "--vault", "store", "--json", "add", "Task")
	if code != 0 {
		t.Fatalf("add: %+v %s", added, stderr)
	}
	id := added["id"].(string)
	want := filepath.Join(cwd, "store", id+".md")
	if added["path"] != want || !filepath.IsAbs(added["path"].(string)) {
		t.Fatalf("add path: %v, want %s", added["path"], want)
	}
	shown, stderr, code := s.run(t, "", "--vault", "store", "--json", "show", id)
	if code != 0 || shown["path"] != want {
		t.Fatalf("show path: %+v %s", shown, stderr)
	}
}

func TestAddListAndShowJSON(t *testing.T) {
	s := newSandbox(t)
	added, stderr, code := s.run(t, "", "--json", "add", "Task", "--tag", "#work", "--body", "body")
	if code != 0 {
		t.Fatalf("add: %+v %s", added, stderr)
	}
	id, _ := added["id"].(string)
	path, _ := added["path"].(string)
	if len(id) != 10 || added["rank"] != float64(0) || path != filepath.Join(s.root, "data", "tt", "project", id+".md") {
		t.Fatalf("added: %+v", added)
	}
	listed, _, code := s.run(t, "", "list", "--json")
	if code != 0 || len(listed["revision"].(string)) != 64 || len(listed["tasks"].([]any)) != 1 {
		t.Fatalf("list: %+v", listed)
	}
	shown, _, code := s.run(t, "", "show", id, "--json")
	if code != 0 || shown["body"] != "body" || shown["path"] != path {
		t.Fatalf("show: %+v", shown)
	}
}
