// Package cli implements the headless tt command-line interface.
package cli

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/spf13/cobra"
	"github.com/vieitesss/tt/internal/project"
	"github.com/vieitesss/tt/internal/vault"
)

const unregistered = "not a registered project; run tt add or tt project add"

type app struct {
	in       io.Reader
	out      io.Writer
	err      io.Writer
	json     bool
	path     string
	vault    string
	reader   *bufio.Reader
	root     *cobra.Command
	terminal bool
}

type usageError struct{ error }

func usage(format string, args ...any) error { return usageError{fmt.Errorf(format, args...)} }

// Run executes one CLI invocation and returns its documented exit code.
func Run(args []string, in io.Reader, out, stderr io.Writer) int {
	a := &app{in: in, out: out, err: stderr, reader: bufio.NewReader(in), terminal: isTerminal(in) && isTerminal(out)}
	root := a.command()
	a.root = root
	root.SetArgs(args)
	root.SetIn(in)
	root.SetOut(out)
	root.SetErr(stderr)
	if err := root.Execute(); err != nil {
		if strings.HasPrefix(err.Error(), "unknown command ") {
			err = usage("%s", err)
		}
		return a.reportError(err)
	}
	return 0
}

func (a *app) reportError(err error) int {
	var invalid usageError
	if errors.As(err, &invalid) {
		fmt.Fprintf(a.err, "error: %s\n\n", invalid.Error())
		if a.root != nil {
			fmt.Fprint(a.err, a.root.UsageString())
		}
		return 2
	}
	var partial *vault.PartialApplyError
	if a.json {
		if errors.As(err, &partial) {
			_ = a.writeJSON(map[string]any{"error": err.Error(), "applied": partial.Applied})
		} else {
			_ = a.writeJSON(map[string]string{"error": err.Error()})
		}
	} else if errors.As(err, &partial) {
		applied, _ := json.Marshal(partial.Applied)
		fmt.Fprintf(a.err, "error: %s; applied: %s\n", err, applied)
	} else {
		fmt.Fprintf(a.err, "error: %s\n", err)
	}
	return 1
}

func (a *app) command() *cobra.Command {
	root := &cobra.Command{
		Use:           "tt",
		Short:         "ToTask — a project task manager",
		Version:       "0.4.2",
		SilenceUsage:  true,
		SilenceErrors: true,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) != 0 {
				return usage("unknown command %q", args[0])
			}
			return cmd.Help()
		},
	}
	root.CompletionOptions.DisableDefaultCmd = true
	root.SetVersionTemplate("{{.Name}} {{.Version}}\n")
	root.SetFlagErrorFunc(func(_ *cobra.Command, err error) error { return usage("%s", err) })
	root.PersistentFlags().BoolVar(&a.json, "json", false, "Emit machine-readable JSON")
	root.PersistentFlags().StringVar(&a.path, "path", "", "Directory to resolve the project from")
	root.PersistentFlags().StringVar(&a.vault, "vault", "", "Store directory")
	_ = root.PersistentFlags().MarkHidden("vault")
	root.Flags().BoolP("version", "V", false, "Print version")
	root.AddCommand(a.addCommand(), a.listCommand(), a.showCommand(), a.stateCommand("done", "done"), a.stateCommand("cancel", "cancelled"), a.stateCommand("reopen", "open"), a.editCommand(), a.moveCommand(), a.projectCommand(), a.applyCommand(), a.updateCommand())
	return root
}

func exactArgs(n int) cobra.PositionalArgs {
	return func(_ *cobra.Command, args []string) error {
		if len(args) != n {
			return usage("expected %d argument(s), got %d", n, len(args))
		}
		return nil
	}
}

func maxArgs(n int) cobra.PositionalArgs {
	return func(_ *cobra.Command, args []string) error {
		if len(args) > n {
			return usage("expected at most %d argument(s), got %d", n, len(args))
		}
		return nil
	}
}

func (a *app) writeJSON(value any) error {
	encoder := json.NewEncoder(a.out)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(value); err != nil {
		return fmt.Errorf("serializing JSON: %w", err)
	}
	return nil
}

func (a *app) loadConfig() (*project.Config, error) {
	config, err := project.Load()
	if err != nil {
		return nil, fmt.Errorf("loading config: %w", err)
	}
	if config.DeprecatedVault != nil {
		fmt.Fprintln(a.err, "warning: the `vault` config key is deprecated and ignored; use `tt project add` to register projects")
	}
	return config, nil
}

func (a *app) start() (string, error) {
	start, err := project.StartDir(a.path)
	if err != nil {
		return "", fmt.Errorf("determining the starting directory: %w", err)
	}
	return start, nil
}

func (a *app) register(config *project.Config, path string) (project.Project, error) {
	p, created, err := config.RegisterAndSave(path)
	if created && project.Nonportable(p.Path) {
		fmt.Fprintf(a.err, "warning: project path %s is outside HOME and will not be portable\n", p.Path)
	}
	return p, err
}

func (a *app) interactive() bool { return !a.json && a.terminal }

func (a *app) ask(prompt string) (string, error) {
	if _, err := fmt.Fprint(a.err, prompt); err != nil {
		return "", fmt.Errorf("flushing the prompt: %w", err)
	}
	answer, err := a.reader.ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return "", fmt.Errorf("reading the answer: %w", err)
	}
	return strings.ToLower(strings.TrimSpace(answer)), nil
}

func (a *app) openVault(config *project.Config, mayRegister bool) (*vault.Vault, error) {
	path := a.vault
	if path == "" {
		path = os.Getenv("TT_VAULT")
	}
	if path != "" {
		v, err := vault.Open(path)
		if err != nil {
			return nil, fmt.Errorf("opening vault %s: %w", path, err)
		}
		return v, nil
	}
	start, err := a.start()
	if err != nil {
		return nil, err
	}
	resolution := project.Resolve(config, start)
	var p project.Project
	if resolution.Project == nil {
		if !mayRegister {
			return nil, errors.New(unregistered)
		}
		if a.interactive() {
			answer, err := a.ask(fmt.Sprintf("register %s as a tt project? [y/N] ", resolution.Path))
			if err != nil {
				return nil, err
			}
			if answer != "y" && answer != "yes" {
				return nil, errors.New(unregistered)
			}
		}
		p, err = a.register(config, resolution.Path)
		if err != nil {
			return nil, err
		}
	} else {
		p = *resolution.Project
		if mayRegister && resolution.Nested != nil && !p.NeverAskNested && a.interactive() {
			answer, err := a.ask(fmt.Sprintf("%s is inside project %s; manage it as a separate project? [y/N/!] (! = never ask inside %s) ", *resolution.Nested, p.Slug, p.Slug))
			if err != nil {
				return nil, err
			}
			switch answer {
			case "y", "yes":
				p, err = a.register(config, *resolution.Nested)
				if err != nil {
					return nil, err
				}
			case "!", "never":
				if err := config.SetNeverAsk(p.Path); err != nil {
					return nil, fmt.Errorf("saving the config: %w", err)
				}
			}
		}
	}
	return project.OpenStore(p)
}

func (a *app) store(mayRegister bool) (*vault.Vault, *project.Config, error) {
	config, err := a.loadConfig()
	if err != nil {
		return nil, nil, err
	}
	v, err := a.openVault(config, mayRegister)
	if err != nil {
		return nil, nil, err
	}
	for _, issue := range v.Issues() {
		fmt.Fprintf(a.err, "warning: %s\n", issue)
	}
	return v, config, nil
}

func parseID(raw string) error {
	if err := vault.ValidateID(raw); err != nil {
		return fmt.Errorf("invalid task id: %s: %w", raw, err)
	}
	return nil
}

func currentProject(config *project.Config, v *vault.Vault) (project.Project, error) {
	data := project.DataDir()
	if data == "" {
		return project.Project{}, errors.New("cannot determine the data directory")
	}
	for _, p := range config.Projects {
		if project.Normalize(project.StorePath(data, p.Slug)) == project.Normalize(v.Root()) {
			return p, nil
		}
	}
	return project.Project{}, errors.New("the current store is not a registered project")
}
