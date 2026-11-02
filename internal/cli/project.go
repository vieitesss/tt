package cli

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/spf13/cobra"
	"github.com/vieitesss/tt/internal/project"
)

type projectRef struct {
	Path string `json:"path"`
	Slug string `json:"slug"`
}

func ref(p project.Project) projectRef { return projectRef{p.Path, p.Slug} }

func (a *app) projectCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use: "project", Short: "List, add, or remove registered projects",
		RunE: func(_ *cobra.Command, _ []string) error { return usage("a project subcommand is required") },
	}
	cmd.AddCommand(&cobra.Command{
		Use: "list", Short: "List registered projects", Args: exactArgs(0),
		RunE: func(_ *cobra.Command, _ []string) error {
			config, err := a.loadConfig()
			if err != nil {
				return err
			}
			if a.json {
				return a.writeJSON(map[string]any{"projects": config.Projects})
			}
			for _, p := range config.Projects {
				fmt.Fprintf(a.out, "%s\t%s\n", p.Slug, p.Path)
			}
			return nil
		},
	})
	cmd.AddCommand(&cobra.Command{
		Use: "current", Short: "Show the resolved project", Args: exactArgs(0),
		RunE: func(_ *cobra.Command, _ []string) error {
			config, err := a.loadConfig()
			if err != nil {
				return err
			}
			start, err := a.start()
			if err != nil {
				return err
			}
			result := project.Resolve(config, start)
			var slug *string
			if result.Project != nil {
				slug = &result.Project.Slug
			}
			if a.json {
				return a.writeJSON(struct {
					Registered bool    `json:"registered"`
					Path       string  `json:"path"`
					Slug       *string `json:"slug"`
					Nested     *string `json:"nested"`
				}{result.Project != nil, result.Path, slug, result.Nested})
			}
			if result.Project == nil {
				fmt.Fprintf(a.out, "unregistered  %s\n", result.Path)
			} else {
				fmt.Fprintf(a.out, "%s\t%s\n", result.Project.Slug, result.Path)
			}
			return nil
		},
	})
	cmd.AddCommand(&cobra.Command{
		Use: "add [DIR]", Short: "Register a directory as a project", Args: maxArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			path := ""
			var err error
			if len(args) == 1 {
				path = args[0]
			} else {
				path, err = a.start()
				if err != nil {
					return err
				}
			}
			info, statErr := os.Stat(path)
			mapped := path
			if len(args) == 1 && statErr == nil && info.IsDir() {
				mapped = project.MapWorktreePath(path)
				if mapped != project.Normalize(path) {
					mappedInfo, err := os.Stat(mapped)
					if err != nil || !mappedInfo.IsDir() {
						return fmt.Errorf("directory does not exist in the main checkout: %s", mapped)
					}
				}
			}
			config, err := a.loadConfig()
			if err != nil {
				return err
			}
			if statErr != nil || !info.IsDir() {
				return fmt.Errorf("project directory does not exist: %s", path)
			}
			if len(args) == 1 {
				path = mapped
			} else {
				path = project.Resolve(config, path).Path
			}
			p, err := a.register(config, path)
			if err != nil {
				return err
			}
			if a.json {
				return a.writeJSON(ref(p))
			}
			fmt.Fprintf(a.out, "registered %s  %s\n", p.Slug, p.Path)
			return nil
		},
	})
	cmd.AddCommand(&cobra.Command{
		Use: "remove [DIR|SLUG]", Short: "Unregister a project", Args: maxArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			config, err := a.loadConfig()
			if err != nil {
				return err
			}
			if len(args) == 0 {
				return fmt.Errorf("a directory or slug is required")
			}
			raw := args[0]
			lookup := project.MapWorktreePath(raw)
			for _, p := range config.Projects {
				if p.Slug == raw {
					lookup = raw
					break
				}
			}
			p, ok := config.Remove(lookup)
			if !ok {
				return fmt.Errorf("not a registered project: %s", raw)
			}
			if err := config.Save(); err != nil {
				return fmt.Errorf("saving the config: %w", err)
			}
			if a.json {
				return a.writeJSON(map[string]any{"removed": p})
			}
			fmt.Fprintf(a.out, "unregistered %s  %s\n", p.Slug, p.Path)
			return nil
		},
	})
	return cmd
}

func (a *app) updateCommand() *cobra.Command {
	return &cobra.Command{
		Use: "update", Aliases: []string{"upgrade"}, Short: "Install the latest main of tt with Go", Args: exactArgs(0),
		RunE: func(_ *cobra.Command, _ []string) error {
			home := os.Getenv("HOME")
			if home == "" {
				return fmt.Errorf("HOME is not set")
			}
			command := exec.Command("go", "install", "github.com/vieitesss/tt/cmd/tt@main")
			command.Env = append(os.Environ(), "GOBIN="+filepath.Join(home, ".local", "bin"))
			command.Stdin, command.Stdout, command.Stderr = a.in, a.out, a.err
			if err := command.Run(); err != nil {
				if _, ok := err.(*exec.ExitError); ok {
					return fmt.Errorf("go install failed (%s)", command.ProcessState)
				}
				return fmt.Errorf("running go; is the Go toolchain installed?: %w", err)
			}
			return nil
		},
	}
}
