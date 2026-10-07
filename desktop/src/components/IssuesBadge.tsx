import type { Issue } from "../types";
import { Disclosure } from "./Disclosure";

/** Store issues stay reported (only when there are any) behind a disclosure. */
export function IssuesBadge({ issues }: { issues: Issue[] }) {
  if (issues.length === 0) return null;
  return (
    <Disclosure
      className="issues"
      label={`⚠ ${issues.length} store issue${issues.length === 1 ? "" : "s"}`}
      title="Store issues are reported, never repaired"
      align="end"
      panelClassName="issue-panel"
    >
      <div className="issue-region" role="region" aria-label="Store issues">
        {issues.map((issue, index) => (
          <div key={`${issue.path}-${index}`} className="issue">
            <span className="issue-kind">{issue.kind}</span>
            <span className="issue-path">{issue.path}</span>
            <span className="issue-detail">{issue.detail}</span>
          </div>
        ))}
      </div>
    </Disclosure>
  );
}
