package project

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/vieitesss/tt/internal/vault"
)

// Normalize makes a path absolute and resolves symlinks when the path exists.
func Normalize(path string) string {
	if resolved, err := filepath.EvalSymlinks(path); err == nil {
		if absolute, err := filepath.Abs(resolved); err == nil {
			return filepath.Clean(absolute)
		}
	}
	if absolute, err := filepath.Abs(path); err == nil {
		return filepath.Clean(absolute)
	}
	return filepath.Clean(path)
}

func StorePath(dataDir, slug string) string { return filepath.Join(dataDir, slug) }

func OpenStore(p Project) (*vault.Vault, error) {
	data := DataDir()
	if data == "" {
		return nil, errors.New("cannot determine the data directory")
	}
	store := StorePath(data, p.Slug)
	info, err := os.Stat(p.Path)
	if err != nil || !info.IsDir() {
		return nil, fmt.Errorf("opening the project store %s: filesystem error at %s: registered project directory is missing", store, p.Path)
	}
	v, err := vault.Open(store)
	if err != nil {
		return nil, fmt.Errorf("opening the project store %s: %w", store, err)
	}
	return v, nil
}

func SlugFor(path string, projects []Project, data string) string {
	base := filepath.Base(path)
	if base == "." || base == string(filepath.Separator) || base == "" {
		base = "project"
	}
	taken := map[string]bool{}
	for _, p := range projects {
		taken[p.Slug] = true
	}
	for n := 1; ; n++ {
		candidate := base
		if n > 1 {
			candidate = fmt.Sprintf("%s-%d", base, n)
		}
		_, err := os.Stat(StorePath(data, candidate))
		if !taken[candidate] && err != nil {
			return candidate
		}
	}
}

// Register creates a Store for an existing directory and records its path.
func (config *Config) Register(path string) (Project, bool, error) {
	path = Normalize(path)
	info, err := os.Stat(path)
	if err != nil || !info.IsDir() {
		return Project{}, false, fmt.Errorf("project directory does not exist: %s", path)
	}
	data := DataDir()
	if data == "" {
		return Project{}, false, errors.New("registering the project: cannot determine the data directory: set $XDG_DATA_HOME or $HOME")
	}
	for _, p := range config.Projects {
		if Normalize(p.Path) == path {
			return p, false, nil
		}
	}
	slug := SlugFor(path, config.Projects, data)
	store := StorePath(data, slug)
	if err := os.MkdirAll(store, 0o777); err != nil {
		return Project{}, false, fmt.Errorf("registering the project: cannot create the store folder %s: %w", store, err)
	}
	p := Project{Path: path, Slug: slug}
	config.Projects = append(config.Projects, p)
	return p, true, nil
}

func (config *Config) RegisterAndSave(path string) (Project, bool, error) {
	p, created, err := config.Register(path)
	if err != nil {
		return Project{}, false, err
	}
	if err := config.Save(); err != nil {
		return Project{}, false, fmt.Errorf("saving the config: %w", err)
	}
	return p, created, nil
}

// SetNeverAsk persists the nested-project preference for a registered path.
// Unregistered paths are ignored without writing the config.
func (config *Config) SetNeverAsk(path string) error {
	path = Normalize(path)
	for i, p := range config.Projects {
		if Normalize(p.Path) == path {
			config.Projects[i].NeverAskNested = true
			return config.Save()
		}
	}
	return nil
}

func (config *Config) Remove(raw string) (Project, bool) {
	path := Normalize(raw)
	for i, p := range config.Projects {
		if Normalize(p.Path) == path {
			config.Projects = append(config.Projects[:i], config.Projects[i+1:]...)
			return p, true
		}
	}
	for i, p := range config.Projects {
		if p.Slug == raw {
			config.Projects = append(config.Projects[:i], config.Projects[i+1:]...)
			return p, true
		}
	}
	return Project{}, false
}

func Nonportable(path string) bool {
	home := os.Getenv("HOME")
	if home == "" {
		return false
	}
	relative, err := filepath.Rel(Normalize(home), Normalize(path))
	return err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) || filepath.IsAbs(relative)
}
