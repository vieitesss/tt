package project

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// Resolution identifies the registered project, or the path to register.
// Nested is the resolved starting directory when strictly below a project.
type Resolution struct {
	Project *Project
	Path    string
	Nested  *string
}

type gitIdentity struct {
	common   string
	gitDir   string
	top      string
	mainRoot string
}

func identifyGit(start string) (gitIdentity, bool) {
	output, err := exec.Command("git", "-C", start, "rev-parse", "--path-format=absolute", "--git-common-dir", "--git-dir", "--show-toplevel").Output()
	if err != nil {
		return gitIdentity{}, false
	}
	lines := strings.Split(strings.TrimSuffix(string(output), "\n"), "\n")
	if len(lines) != 3 {
		return gitIdentity{}, false
	}
	common := Normalize(lines[0])
	gitDir := Normalize(lines[1])
	top := Normalize(lines[2])
	root := common
	if filepath.Base(common) == ".git" {
		root = filepath.Dir(common)
	}
	return gitIdentity{common: common, gitDir: gitDir, top: top, mainRoot: root}, true
}

func isWithin(path, root string) bool {
	relative, err := filepath.Rel(root, path)
	return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) && !filepath.IsAbs(relative)
}

func structural(config *Config, start string, lowerBound string) Resolution {
	for ancestor := start; ; ancestor = filepath.Dir(ancestor) {
		if lowerBound != "" && !isWithin(ancestor, lowerBound) {
			break
		}
		for _, p := range config.Projects {
			if Normalize(p.Path) == ancestor {
				var nested *string
				if start != ancestor {
					value := start
					nested = &value
				}
				p := p
				return Resolution{Project: &p, Path: p.Path, Nested: nested}
			}
		}
		next := filepath.Dir(ancestor)
		if next == ancestor {
			break
		}
	}
	return Resolution{Path: start}
}

// Resolve gives explicit in-worktree registrations precedence, then maps a
// linked worktree to the corresponding path in its main repository.
func Resolve(config *Config, start string) Resolution {
	start = Normalize(start)
	git, ok := identifyGit(start)
	if !ok {
		return structural(config, start, "")
	}
	if found := structural(config, start, git.top); found.Project != nil {
		return found
	}
	resolvedStart := start
	if git.gitDir != git.common {
		relative, err := filepath.Rel(git.top, start)
		if err == nil {
			resolvedStart = Normalize(filepath.Join(git.mainRoot, relative))
		}
	}
	if found := structural(config, resolvedStart, ""); found.Project != nil {
		return found
	}
	return Resolution{Path: git.mainRoot}
}

// MapWorktreePath maps an explicit worktree directory to its main-root
// equivalent while retaining its relative suffix. Non-worktree paths stay exact.
func MapWorktreePath(path string) string {
	path = Normalize(path)
	if git, ok := identifyGit(path); ok && git.gitDir != git.common {
		if relative, err := filepath.Rel(git.top, path); err == nil {
			return Normalize(filepath.Join(git.mainRoot, relative))
		}
	}
	return path
}

func StartDir(explicit string) (string, error) {
	if explicit != "" {
		return explicit, nil
	}
	if env := os.Getenv("TT_PATH"); env != "" {
		return env, nil
	}
	return os.Getwd()
}
