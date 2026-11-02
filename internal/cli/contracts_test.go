package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

type cliSandbox struct {
	t       *testing.T
	root    string
	project string
	config  string
	data    string
}

func sandbox(t *testing.T) *cliSandbox {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	project := filepath.Join(root, "project")
	if err := os.MkdirAll(project, 0o755); err != nil {
		t.Fatal(err)
	}
	config := filepath.Join(root, "config.toml")
	data := filepath.Join(root, "data")
	t.Setenv("TT_CONFIG", config)
	t.Setenv("XDG_DATA_HOME", data)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "legacy"))
	t.Setenv("HOME", root)
	t.Setenv("TT_PATH", "")
	t.Setenv("TT_VAULT", "")
	return &cliSandbox{t, root, project, config, data}
}

func (s *cliSandbox) run(input string, args ...string) (int, string, string) {
	s.t.Helper()
	var stdout, stderr bytes.Buffer
	args = append([]string{"--path", s.project}, args...)
	code := Run(args, strings.NewReader(input), &stdout, &stderr)
	return code, stdout.String(), stderr.String()
}

func (s *cliSandbox) json(args ...string) map[string]any {
	s.t.Helper()
	code, stdout, stderr := s.run("", append([]string{"--json"}, args...)...)
	if code != 0 {
		s.t.Fatalf("%v: exit %d: stdout %q stderr %q", args, code, stdout, stderr)
	}
	var value map[string]any
	if err := json.Unmarshal([]byte(stdout), &value); err != nil {
		s.t.Fatalf("%v: invalid JSON %q: %v", args, stdout, err)
	}
	return value
}

func (s *cliSandbox) add(title string, extra ...string) string {
	s.t.Helper()
	args := append([]string{"add", "--title", title}, extra...)
	return s.json(args...)["id"].(string)
}

func TestErrorsUseRuntimeAndUsageContracts(t *testing.T) {
	s := sandbox(t)
	for _, tc := range []struct {
		name string
		args []string
		code int
		want string
		json bool
	}{
		{"unregistered json", []string{"--json", "list"}, 1, unregistered, true},
		{"unregistered human", []string{"list"}, 1, unregistered, false},
		{"unknown flag", []string{"--json", "list", "--unknown"}, 2, "Usage:", false},
		{"invalid enum", []string{"--json", "list", "--state", "wrong"}, 2, "Usage:", false},
		{"missing argument", []string{"--json", "show"}, 2, "Usage:", false},
		{"conflicting move", []string{"--json", "move", "a", "--parent", "b", "--root"}, 2, "cannot be used with", false},
		{"missing move flag", []string{"--json", "move", "a"}, 2, "required arguments", false},
		{"unknown command", []string{"--json", "bogus"}, 2, "Usage:", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			code, stdout, stderr := s.run("", tc.args...)
			if code != tc.code {
				t.Fatalf("exit %d, want %d; stdout=%q stderr=%q", code, tc.code, stdout, stderr)
			}
			if tc.json {
				var value map[string]any
				if json.Unmarshal([]byte(stdout), &value) != nil || value["error"] != tc.want || stderr != "" {
					t.Fatalf("bad JSON error: %q, stderr %q", stdout, stderr)
				}
			} else if !strings.Contains(stderr, tc.want) || stdout != "" || !strings.HasPrefix(stderr, "error: ") {
				t.Fatalf("bad stderr: %q, stdout %q", stderr, stdout)
			}
		})
	}
}

func TestPartialApplyErrorReportsProgressInBothModes(t *testing.T) {
	partial := &vault.PartialApplyError{Cause: os.ErrPermission, Applied: vault.ApplyPlan{
		Revision: "new", Adds: []vault.ApplyAdd{{ID: "added", Title: "Added"}},
		Updates: []vault.ApplyUpdate{}, Moves: []vault.ApplyMove{}, Deletes: []vault.ApplyDelete{},
	}}
	for _, jsonMode := range []bool{false, true} {
		var out, stderr bytes.Buffer
		a := &app{out: &out, err: &stderr, json: jsonMode}
		if got := a.reportError(partial); got != 1 {
			t.Fatalf("exit %d", got)
		}
		if jsonMode {
			var value struct {
				Error   string          `json:"error"`
				Applied vault.ApplyPlan `json:"applied"`
			}
			if err := json.Unmarshal(out.Bytes(), &value); err != nil || value.Error != partial.Error() || len(value.Applied.Adds) != 1 || value.Applied.Adds[0].ID != "added" || stderr.Len() != 0 {
				t.Fatalf("partial JSON %q stderr %q: %v", out.String(), stderr.String(), err)
			}
		} else if out.Len() != 0 || !strings.Contains(stderr.String(), "permission denied") || !strings.Contains(stderr.String(), "added") {
			t.Fatalf("partial human report stdout %q stderr %q", out.String(), stderr.String())
		}
	}
}
