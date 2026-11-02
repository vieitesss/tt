package cli

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"

	"github.com/spf13/cobra"
	"github.com/vieitesss/tt/internal/vault"
)

func (a *app) applyCommand() *cobra.Command {
	var dryRun bool
	cmd := &cobra.Command{
		Use: "apply", Short: "Apply a declarative task tree", Args: exactArgs(0),
		RunE: func(_ *cobra.Command, _ []string) error {
			var input struct {
				Revision *string              `json:"revision"`
				Tasks    *[]vault.DesiredTask `json:"tasks"`
			}
			decoder := json.NewDecoder(a.in)
			if err := decoder.Decode(&input); err != nil {
				return fmt.Errorf("reading apply input: %w", err)
			}
			var extra any
			if err := decoder.Decode(&extra); !errors.Is(err, io.EOF) {
				if err == nil {
					return errors.New("reading apply input: expected one JSON object")
				}
				return fmt.Errorf("reading apply input: %w", err)
			}
			if input.Revision == nil || input.Tasks == nil {
				return errors.New("reading apply input: revision and tasks are required")
			}
			v, _, err := a.store(false)
			if err != nil {
				return err
			}
			plan, err := v.Apply(vault.ApplyInput{Revision: *input.Revision, Tasks: *input.Tasks}, dryRun)
			if err != nil {
				return err
			}
			if a.json {
				return a.writeJSON(plan)
			}
			label := "applied"
			if dryRun {
				label = "planned"
			}
			fmt.Fprintf(a.out, "%s: %d adds, %d updates, %d moves, %d reorders, %d deletes\n", label, len(plan.Adds), len(plan.Updates), len(plan.Moves), plan.Reorders, len(plan.Deletes))
			return nil
		},
	}
	cmd.Flags().BoolVar(&dryRun, "dry-run", false, "Plan without writing")
	return cmd
}
