package cli

import (
	"fmt"

	"github.com/spf13/cobra"
	"github.com/vieitesss/tt/internal/project"
	"github.com/vieitesss/tt/internal/vault"
)

func registeredDestination(config *project.Config, raw string) (project.Project, error) {
	for _, p := range config.Projects {
		if p.Slug == raw {
			return p, nil
		}
	}
	mapped := project.MapWorktreePath(raw)
	for _, p := range config.Projects {
		if project.Normalize(p.Path) == mapped {
			return p, nil
		}
	}
	return project.Project{}, fmt.Errorf("not a registered project: %s", raw)
}

func (a *app) moveCommand() *cobra.Command {
	var toProject, parent string
	var root bool
	cmd := &cobra.Command{
		Use: "move ID", Short: "Reparent a task or move its subtree to another project",
		Args: func(cmd *cobra.Command, args []string) error {
			if err := exactArgs(1)(cmd, args); err != nil {
				return err
			}
			if root && cmd.Flags().Changed("parent") {
				return usage("--parent cannot be used with --root")
			}
			if !root && !cmd.Flags().Changed("parent") && !cmd.Flags().Changed("to-project") {
				return usage("the required arguments --parent or --root were not provided")
			}
			return nil
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			source, config, err := a.store(false)
			if err != nil {
				return err
			}
			id := args[0]
			if err := parseID(id); err != nil {
				return err
			}
			var selectedParent *string
			if cmd.Flags().Changed("parent") {
				if err := parseID(parent); err != nil {
					return err
				}
				selectedParent = &parent
			}
			var targetProject project.Project
			var target *vault.Vault
			if cmd.Flags().Changed("to-project") {
				targetProject, err = registeredDestination(config, toProject)
				if err != nil {
					return err
				}
				target, err = project.OpenStore(targetProject)
				if err != nil {
					return err
				}
			} else {
				targetProject, err = currentProject(config, source)
				if err != nil {
					return err
				}
				target = source
			}
			var tasks []vault.Task
			if project.Normalize(source.Root()) == project.Normalize(target.Root()) {
				if _, err = source.SetParent(id, selectedParent); err != nil {
					return fmt.Errorf("moving the task subtree: %w", err)
				}
				tasks = source.Subtree(id)
			} else {
				for _, issue := range target.Issues() {
					fmt.Fprintf(a.err, "warning: %s\n", issue)
				}
				tasks, err = source.MoveTo(target, []string{id}, selectedParent)
				if err != nil {
					return fmt.Errorf("moving the task subtree: %w", err)
				}
			}
			if a.json {
				return a.writeJSON(struct {
					Tasks         []vault.Task `json:"tasks"`
					TargetProject projectRef   `json:"target_project"`
				}{tasks, ref(targetProject)})
			}
			plural := "tasks"
			if len(tasks) == 1 {
				plural = "task"
			}
			fmt.Fprintf(a.out, "moved %d %s to %s\n", len(tasks), plural, targetProject.Slug)
			return nil
		},
	}
	cmd.Flags().StringVar(&toProject, "to-project", "", "Registered destination directory or slug")
	cmd.Flags().StringVar(&parent, "parent", "", "Destination parent task id")
	cmd.Flags().BoolVar(&root, "root", false, "Make the task a root")
	return cmd
}
