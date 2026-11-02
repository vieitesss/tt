package cli

import (
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"
	"github.com/vieitesss/tt/internal/vault"
)

type taskWithPath struct {
	vault.Task
	Path string `json:"path"`
}

type taskNode struct {
	vault.Task
	Children []taskNode `json:"children"`
}

func nodeJSON(node vault.Node) taskNode {
	children := make([]taskNode, 0, len(node.Children))
	for _, child := range node.Children {
		children = append(children, nodeJSON(child))
	}
	return taskNode{Task: node.Task, Children: children}
}

func (a *app) printTask(task vault.Task, prefix string) error {
	if a.json {
		return a.writeJSON(task)
	}
	_, err := fmt.Fprintf(a.out, "%s %s  %s\n", prefix, task.ID, task.Title)
	return err
}

func (a *app) addCommand() *cobra.Command {
	var title, parent, due, priority, body string
	var tags []string
	cmd := &cobra.Command{
		Use:   "add [TITLE]",
		Short: "Create a task",
		Args: func(cmd *cobra.Command, args []string) error {
			if len(args) > 1 || len(args) == 1 && cmd.Flags().Changed("title") {
				return usage("pass either a positional title or --title")
			}
			if cmd.Flags().Changed("priority") && !vault.ValidPriority(priority) {
				return usage("invalid priority %q; expected high, med, or low", priority)
			}
			return nil
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			v, config, err := a.store(true)
			if err != nil {
				return err
			}
			if !cmd.Flags().Changed("title") {
				if len(args) == 0 {
					return fmt.Errorf("a title is required: pass `--title` or a positional title")
				}
				title = args[0]
			}
			selectedParent := config.CaptureTarget
			if cmd.Flags().Changed("parent") {
				if err := parseID(parent); err != nil {
					return err
				}
				selectedParent = &parent
			}
			var date, prio *string
			if cmd.Flags().Changed("due") {
				if err := vault.ValidateDate(due); err != nil {
					return err
				}
				date = &due
			}
			if cmd.Flags().Changed("priority") {
				prio = &priority
			}
			if body == "-" {
				data, err := io.ReadAll(a.reader)
				if err != nil {
					return fmt.Errorf("reading body from stdin: %w", err)
				}
				body = string(data)
			}
			task, err := v.Add(vault.AddInput{Title: title, Parent: selectedParent, Tags: tags, Due: date, Priority: prio, Body: body})
			if err != nil {
				return fmt.Errorf("adding the task: %w", err)
			}
			if a.json {
				path, err := filepath.Abs(v.Path(task.ID))
				if err != nil {
					return fmt.Errorf("determining absolute task path: %w", err)
				}
				return a.writeJSON(taskWithPath{Task: task, Path: path})
			}
			return a.printTask(task, "added")
		},
	}
	cmd.Flags().StringVar(&title, "title", "", "Task title")
	cmd.Flags().StringVar(&parent, "parent", "", "Parent task id")
	cmd.Flags().StringArrayVar(&tags, "tag", nil, "Tag (repeatable)")
	cmd.Flags().StringVar(&due, "due", "", "Due date (YYYY-MM-DD)")
	cmd.Flags().StringVar(&priority, "priority", "", "Priority: high, med, or low")
	cmd.Flags().StringVar(&body, "body", "", "Body markdown, or - for stdin")
	return cmd
}

func (a *app) listCommand() *cobra.Command {
	var flat, dueToday bool
	var tag, state string
	cmd := &cobra.Command{
		Use:   "list",
		Short: "List tasks as a tree or a flat list",
		Args: func(cmd *cobra.Command, args []string) error {
			if err := exactArgs(0)(cmd, args); err != nil {
				return err
			}
			if cmd.Flags().Changed("state") && !vault.ValidState(state) {
				return usage("invalid state %q; expected open, done, or cancelled", state)
			}
			return nil
		},
		RunE: func(cmd *cobra.Command, _ []string) error {
			v, _, err := a.store(false)
			if err != nil {
				return err
			}
			filter := vault.Filter{DueToday: dueToday}
			if cmd.Flags().Changed("tag") {
				filter.Tag = &tag
			}
			if cmd.Flags().Changed("state") {
				filter.State = &state
			}
			if flat {
				tasks := v.Filtered(filter, time.Now())
				if a.json {
					return a.writeJSON(map[string]any{"tasks": tasks, "revision": v.SnapshotRevision()})
				}
				for _, task := range tasks {
					fmt.Fprintln(a.out, humanLine(task))
				}
				return nil
			}
			tree := v.Tree(filter, time.Now())
			if a.json {
				nodes := make([]taskNode, 0, len(tree))
				for _, node := range tree {
					nodes = append(nodes, nodeJSON(node))
				}
				return a.writeJSON(map[string]any{"tasks": nodes, "revision": v.SnapshotRevision()})
			}
			var printTree func(vault.Node, int)
			printTree = func(node vault.Node, depth int) {
				fmt.Fprintln(a.out, strings.Repeat("  ", depth)+humanLine(node.Task))
				for _, child := range node.Children {
					printTree(child, depth+1)
				}
			}
			for _, node := range tree {
				printTree(node, 0)
			}
			return nil
		},
	}
	cmd.Flags().BoolVar(&flat, "flat", false, "Flat list instead of a tree")
	cmd.Flags().StringVar(&tag, "tag", "", "Only tasks carrying this tag")
	cmd.Flags().StringVar(&state, "state", "", "Only tasks in this state")
	cmd.Flags().BoolVar(&dueToday, "due-today", false, "Only tasks due today or earlier")
	return cmd
}

func humanLine(task vault.Task) string {
	marker := "[ ]"
	if task.State == "done" {
		marker = "[x]"
	} else if task.State == "cancelled" {
		marker = "[-]"
	}
	return fmt.Sprintf("%s %s  %s", marker, task.ID, task.Title)
}

func (a *app) showCommand() *cobra.Command {
	return &cobra.Command{
		Use: "show ID", Short: "Show one task in detail", Args: exactArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			v, _, err := a.store(false)
			if err != nil {
				return err
			}
			id := args[0]
			if err := parseID(id); err != nil {
				return err
			}
			task, ok := v.Get(id)
			if !ok {
				return fmt.Errorf("task not found: %s", id)
			}
			if a.json {
				path, err := filepath.Abs(v.Path(task.ID))
				if err != nil {
					return fmt.Errorf("determining absolute task path: %w", err)
				}
				return a.writeJSON(struct {
					vault.Task
					Body      string   `json:"body"`
					Links     []string `json:"links"`
					Backlinks []string `json:"backlinks"`
					Children  []string `json:"children"`
					Path      string   `json:"path"`
				}{task, task.Body, v.Links(id), v.Backlinks(id), v.Children(id), path})
			}
			fmt.Fprintln(a.out, humanShow(v, task))
			return nil
		},
	}
}

func describeIDs(v *vault.Vault, ids []string) string {
	if len(ids) == 0 {
		return "-"
	}
	parts := []string{}
	for _, id := range ids {
		if task, ok := v.Get(id); ok {
			parts = append(parts, fmt.Sprintf("%s (%s)", task.Title, id))
		} else {
			parts = append(parts, id)
		}
	}
	return strings.Join(parts, ", ")
}

func humanShow(v *vault.Vault, task vault.Task) string {
	parent := "-"
	if task.Parent != nil {
		parent = describeIDs(v, []string{*task.Parent})
	}
	tags := "-"
	if len(task.Tags) != 0 {
		tags = strings.Join(task.Tags, ", ")
	}
	due, priority := "-", "-"
	if task.Due != nil {
		due = *task.Due
	}
	if task.Priority != nil {
		priority = *task.Priority
	}
	lines := []string{task.Title, "id: " + task.ID, "state: " + task.State, "parent: " + parent, "tags: " + tags, "due: " + due, "priority: " + priority, "children: " + describeIDs(v, v.Children(task.ID)), "links: " + describeIDs(v, v.Links(task.ID)), "backlinks: " + describeIDs(v, v.Backlinks(task.ID))}
	if strings.TrimSpace(task.Body) != "" {
		lines = append(lines, "", strings.TrimRight(task.Body, " \t\r\n"))
	}
	return strings.Join(lines, "\n")
}

func (a *app) stateCommand(name, state string) *cobra.Command {
	return &cobra.Command{
		Use: name + " ID", Short: "Set task state", Args: exactArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			v, _, err := a.store(false)
			if err != nil {
				return err
			}
			if err := parseID(args[0]); err != nil {
				return err
			}
			task, err := v.SetState(args[0], state)
			if err != nil {
				return err
			}
			return a.printTask(task, state)
		},
	}
}

func (a *app) editCommand() *cobra.Command {
	var title string
	cmd := &cobra.Command{
		Use: "edit ID", Short: "Rename a task, or open it in $EDITOR", Args: exactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			v, _, err := a.store(false)
			if err != nil {
				return err
			}
			id := args[0]
			if err := parseID(id); err != nil {
				return err
			}
			if cmd.Flags().Changed("title") {
				task, _, err := v.SetTitle(id, title)
				if err != nil {
					return err
				}
				return a.printTask(task, "edited")
			}
			if _, ok := v.Get(id); !ok {
				return fmt.Errorf("task not found: %s", id)
			}
			parts := strings.Fields(os.Getenv("EDITOR"))
			if len(parts) == 0 {
				parts = []string{"vi"}
			}
			command := exec.Command(parts[0], append(parts[1:], v.Path(id))...)
			command.Stdin, command.Stdout, command.Stderr = a.in, a.out, a.err
			if err := command.Run(); err != nil {
				var exit *exec.ExitError
				if errors.As(err, &exit) {
					return fmt.Errorf("editor exited with %s", exit.ProcessState.String())
				}
				return fmt.Errorf("launching `%s` for %s: %w", parts[0], v.Path(id), err)
			}
			if err := v.Reload(); err != nil {
				return err
			}
			task, ok := v.Get(id)
			if !ok {
				return fmt.Errorf("task not found after editing: %s", id)
			}
			return a.printTask(task, "edited")
		},
	}
	cmd.Flags().StringVar(&title, "title", "", "New title")
	return cmd
}
