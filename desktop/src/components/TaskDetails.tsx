import { Fragment, useEffect, useMemo, useState } from "react";

import type { TtApp } from "../hooks/useTtApp";
import { dueTone, formatDue, STATE_GLYPH, STATE_LABEL, subtreeIds, todayIso } from "../lib/display";
import { flattenTree } from "../lib/tree";
import type { Priority, TaskRef } from "../types";
import { Disclosure } from "./Disclosure";
import { MarkdownPreview } from "./MarkdownPreview";

export interface TaskDetailsProps {
  app: TtApp;
  onExternal: (url: string) => void;
}

const PRIORITY_LABEL: Record<Priority, string> = { high: "High", med: "Medium", low: "Low" };

/** A backlink/link/sub-task list, rendered only inside a named disclosure. */
function RefList({ refs, onOpen }: { refs: TaskRef[]; onOpen: (id: string) => void }) {
  return (
    <ul className="ref-list">
      {refs.map((ref) => (
        <li key={ref.id}>
          <button
            type="button"
            className={ref.title ? "ref-link" : "ref-link dangling"}
            onClick={() => onOpen(ref.id)}
            title={ref.title ? `Go to ${ref.title}` : `Unresolved reference → ${ref.id}`}
          >
            {ref.title ?? ref.id}
          </button>
        </li>
      ))}
    </ul>
  );
}

/**
 * Selected-task pane. The default view is a readable task: completion control,
 * plain title, populated metadata as quiet text, and the rendered description.
 * Forms and commands live behind named disclosures; disk identity is secondary.
 */
export function TaskDetails({ app, onExternal }: TaskDetailsProps) {
  const { detail, draft, conflict, busy, dirty, saving } = app;
  const [titleDraft, setTitleDraft] = useState("");
  const [tagsDraft, setTagsDraft] = useState("");
  const [moveProject, setMoveProject] = useState("");
  const [moveParent, setMoveParent] = useState("");

  const detailId = detail?.id ?? null;
  const detailTitle = detail?.title ?? "";
  const detailTags = detail?.tags.join(",") ?? "";
  const detailProjectSlug = detail?.projectSlug ?? "";
  useEffect(() => {
    setTitleDraft(detailTitle);
    setTagsDraft(detail?.tags.join(", ") ?? "");
    setMoveProject(detailProjectSlug);
    setMoveParent("");
    // Only re-sync when the selected task or its stored metadata changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detailId, detailTitle, detailTags, detailProjectSlug]);

  const titles = useMemo(() => {
    const map = new Map<string, string>();
    const walk = (nodes: typeof app.tree): void => {
      for (const node of nodes) {
        map.set(node.id, node.title);
        walk(node.children);
      }
    };
    walk(app.tree);
    return map;
  }, [app.tree]);

  const parentOptions = useMemo(() => {
    if (!detail) return [];
    const excluded = subtreeIds(app.tree, detail.id);
    return flattenTree(app.tree, new Set())
      .filter(({ node }) => !excluded.has(node.id))
      .map(({ node, depth }) => ({ id: node.id, label: `${"— ".repeat(depth)}${node.title}` }));
  }, [app.tree, detail]);

  const locked = busy || dirty || app.selectedMissing;

  if (!detail) {
    return (
      <section className="details" aria-label="Task details">
        <div className="empty">
          <h2>No task selected</h2>
          <p>Pick a task on the left to read or edit it.</p>
        </div>
      </section>
    );
  }

  const tags = tagsDraft
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
  const done = detail.state === "done";
  const completeLabel = done || detail.state === "cancelled" ? "Reopen task" : "Mark task done";
  const today = todayIso();

  // Quiet metadata: only what the task actually has.
  const meta: { key: string; text: string; className: string }[] = [];
  if (detail.priority) {
    meta.push({
      key: "priority",
      text: PRIORITY_LABEL[detail.priority],
      className: `prio prio-${detail.priority}`,
    });
  }
  if (detail.due) {
    meta.push({
      key: "due",
      text: `Due ${formatDue(detail.due)}`,
      className: `due tone-${dueTone(detail.due, today)}`,
    });
  }
  for (const tag of detail.tags) meta.push({ key: `tag:${tag}`, text: `#${tag}`, className: "tag" });

  return (
    <section className="details" aria-label="Task details">
      <div className="detail-head">
        <div className="detail-title-row">
          <button
            type="button"
            className={`state-toggle state-${detail.state}`}
            aria-pressed={done}
            aria-label={completeLabel}
            title={STATE_LABEL[detail.state]}
            disabled={locked}
            onClick={() => app.setState(done || detail.state === "cancelled" ? "open" : "done")}
          >
            {STATE_GLYPH[detail.state]}
          </button>
          <h2 className="detail-title" data-testid="detail-title">
            {detail.title}
          </h2>
        </div>

        <div className="detail-actions">
          <Disclosure
            label="Properties"
            title="Title, priority, due date, tags"
            align="end"
            resetKey={detail.id}
            panelClassName="properties-panel"
          >
            <div className="property">
              <span className="property-label">Title</span>
              <input
                aria-label="Task title"
                value={titleDraft}
                disabled={locked}
                onChange={(event) => setTitleDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (
                    event.key === "Enter" &&
                    !locked &&
                    titleDraft.trim() &&
                    titleDraft !== detail.title
                  ) {
                    app.setTitle(titleDraft.trim());
                  }
                }}
              />
              <button
                type="button"
                disabled={locked || !titleDraft.trim() || titleDraft === detail.title}
                onClick={() => app.setTitle(titleDraft.trim())}
              >
                Rename
              </button>
            </div>

            <div className="property">
              <span className="property-label">Priority</span>
              <select
                aria-label="Priority"
                value={detail.priority ?? ""}
                disabled={locked}
                onChange={(event) =>
                  app.setPriority((event.target.value || undefined) as Priority | undefined)
                }
              >
                <option value="">none</option>
                <option value="high">high</option>
                <option value="med">med</option>
                <option value="low">low</option>
              </select>
            </div>

            <div className="property">
              <span className="property-label">Due</span>
              <input
                type="date"
                aria-label="Due date"
                value={detail.due ?? ""}
                disabled={locked}
                onChange={(event) => app.setDue(event.target.value || undefined)}
              />
            </div>

            <div className="property">
              <span className="property-label">Tags</span>
              <input
                aria-label="Tags"
                placeholder="comma, separated"
                value={tagsDraft}
                disabled={locked}
                onChange={(event) => setTagsDraft(event.target.value)}
              />
              <button
                type="button"
                disabled={locked || tags.join(",") === detail.tags.join(",")}
                onClick={() => app.setTags(tags)}
              >
                Apply tags
              </button>
            </div>
          </Disclosure>

          <Disclosure
            label="More actions"
            align="end"
            resetKey={detail.id}
            panelClassName="actions-panel"
          >
            <div className="action-group">
              <h3>Copy</h3>
              <button type="button" disabled={busy} onClick={() => void app.copyMetadata(false)}>
                Copy metadata
              </button>
              <button type="button" disabled={busy} onClick={() => void app.copyMetadata(true)}>
                Copy task
              </button>
            </div>

            <div className="action-group">
              <h3>Order</h3>
              <button type="button" disabled={locked} onClick={() => app.shiftRank(-1)}>
                Move up
              </button>
              <button type="button" disabled={locked} onClick={() => app.shiftRank(1)}>
                Move down
              </button>
            </div>

            <div className="action-group">
              <h3>Move</h3>
              <label>
                Project
                <select
                  aria-label="Move to project"
                  value={moveProject}
                  disabled={locked}
                  onChange={(event) => {
                    setMoveProject(event.target.value);
                    // Parent choices belong to a Project; a cross-project move
                    // lands at the target root unless a target parent is picked.
                    setMoveParent("");
                  }}
                >
                  {app.projects.map((project) => (
                    <option key={project.slug} value={project.slug}>
                      {project.name} (#{project.slug})
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Parent
                <select
                  aria-label="Move under parent"
                  value={moveParent}
                  disabled={locked || moveProject !== detail.projectSlug}
                  onChange={(event) => setMoveParent(event.target.value)}
                >
                  <option value="">— project root —</option>
                  {parentOptions.map((option) => (
                    <option key={option.id} value={option.id}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              <button
                type="button"
                disabled={locked}
                onClick={() => {
                  if (moveProject === detail.projectSlug) {
                    app.setParent(moveParent || undefined);
                  } else {
                    app.moveToProject(moveProject, moveParent || undefined);
                  }
                }}
              >
                Move
              </button>
            </div>

            <div className="action-group">
              <h3>Task</h3>
              {detail.state !== "cancelled" ? (
                <button type="button" disabled={locked} onClick={() => app.setState("cancelled")}>
                  Cancel task
                </button>
              ) : (
                <button type="button" disabled={locked} onClick={() => app.setState("open")}>
                  Reopen task
                </button>
              )}
              <button
                type="button"
                className="danger"
                disabled={busy}
                onClick={() => void app.deleteTask()}
              >
                Delete…
              </button>
            </div>

            <div className="action-group file-group">
              <h3>File</h3>
              <span className="file-path">{detail.filePath}</span>
              <span className="file-id">
                {detail.projectSlug} · {detail.id}
              </span>
            </div>
          </Disclosure>
        </div>
      </div>

      {app.selectedMissing && (
        <div className="conflict" role="alert" data-testid="missing-task">
          <strong>This task is no longer in the project store.</strong>
          <p>
            It was deleted outside the app, or its project was unregistered. Your draft is kept
            here so you can recover it.
          </p>
          <div className="row">
            {draft && (
              <button type="button" onClick={() => void app.copyDraft()}>
                Copy draft
              </button>
            )}
            {draft && (
              <button type="button" className="danger" onClick={app.discardDraft}>
                Discard draft
              </button>
            )}
            <button type="button" onClick={() => void app.clearSelection()}>
              Close
            </button>
          </div>
        </div>
      )}

      {meta.length > 0 && (
        <div className="detail-meta-quiet" data-testid="detail-meta">
          {meta.map((part, index) => (
            <Fragment key={part.key}>
              {index > 0 && (
                <span className="meta-sep" aria-hidden="true">
                  ·
                </span>
              )}
              <span className={part.className}>{part.text}</span>
            </Fragment>
          ))}
        </div>
      )}

      {draft ? (
        <div className="editor">
          {conflict && (
            <div className="conflict" role="alert" data-testid="conflict">
              <strong>
                {conflict.reason === "deleted"
                  ? "This task file was deleted outside the app."
                  : "This task changed on disk while you were editing."}
              </strong>
              <p>Your draft is kept. Reload the latest version, copy the draft, or discard it.</p>
              <div className="row">
                <button type="button" onClick={() => void app.reloadDraft()}>
                  Reload latest
                </button>
                <button type="button" onClick={() => void app.copyDraft()}>
                  Copy draft
                </button>
                <button type="button" className="danger" onClick={() => void app.cancelEdit()}>
                  Discard draft
                </button>
              </div>
            </div>
          )}
          <textarea
            className="body-editor"
            aria-label="Task description"
            spellCheck={false}
            disabled={saving}
            value={draft.text}
            onChange={(event) => app.setDraftText(event.target.value)}
          />
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={busy || saving}
              onClick={() => void app.saveDraft()}
            >
              Save
            </button>
            <button type="button" disabled={busy || saving} onClick={() => void app.cancelEdit()}>
              Cancel
            </button>
            <span className="muted">Baseline {draft.baselineRevision.slice(0, 8)}</span>
          </div>
        </div>
      ) : (
        <>
          <div className="body-preview" data-testid="body-preview">
            {detail.body.trim() === "" ? (
              <p className="muted">[empty description]</p>
            ) : (
              <MarkdownPreview
                body={detail.body}
                resolveTitle={(id) => titles.get(id)}
                onNavigate={(id) => void app.selectTask(id)}
                onExternal={onExternal}
              />
            )}
          </div>
          <div className="description-bar">
            <button type="button" className="quiet" disabled={locked} onClick={app.startEdit}>
              Edit description
            </button>
          </div>
        </>
      )}

      {(detail.children.length > 0 ||
        detail.links.length > 0 ||
        detail.backlinks.length > 0) && (
        <div className="detail-refs">
          {detail.children.length > 0 && (
            <Disclosure
              label={`Sub-tasks (${detail.children.length})`}
              resetKey={detail.id}
              panelClassName="ref-panel"
            >
              <RefList refs={detail.children} onOpen={(id) => void app.selectTask(id)} />
            </Disclosure>
          )}
          {detail.links.length > 0 && (
            <Disclosure
              label={`Links (${detail.links.length})`}
              resetKey={detail.id}
              panelClassName="ref-panel"
            >
              <RefList refs={detail.links} onOpen={(id) => void app.selectTask(id)} />
            </Disclosure>
          )}
          {detail.backlinks.length > 0 && (
            <Disclosure
              label={`Backlinks (${detail.backlinks.length})`}
              resetKey={detail.id}
              panelClassName="ref-panel"
            >
              <RefList refs={detail.backlinks} onOpen={(id) => void app.selectTask(id)} />
            </Disclosure>
          )}
        </div>
      )}
    </section>
  );
}
