import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { TtError, tauriClient } from "./api";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

describe("tauriClient error boundary", () => {
  it("normalizes a serialized ServiceError rejection into a TtError", async () => {
    const serialized = {
      code: "stale_draft",
      message: "the task file changed on disk",
      conflict: { reason: "changed", currentRevision: "rev2" },
    };
    mockInvoke.mockRejectedValue(serialized);

    const failure = await tauriClient.saveBody("a", "draft", "rev1").catch((error) => error);

    expect(failure).toBeInstanceOf(TtError);
    expect(failure).toMatchObject({
      code: "stale_draft",
      message: "the task file changed on disk",
      conflict: { reason: "changed", currentRevision: "rev2" },
    });
  });

  it("keeps a meaningful message for non-conflict native failures", async () => {
    mockInvoke.mockRejectedValue({ code: "io_error", message: "could not write the task file" });

    const failure = await tauriClient.taskDetail("a").catch((error) => error);

    expect(failure).toBeInstanceOf(TtError);
    expect((failure as TtError).code).toBe("io_error");
    expect((failure as TtError).message).toBe("could not write the task file");
  });

  it("passes successful invokes through unchanged", async () => {
    mockInvoke.mockResolvedValue({ id: "a", body: "hello" });

    await expect(tauriClient.taskDetail("a")).resolves.toEqual({ id: "a", body: "hello" });
    expect(mockInvoke).toHaveBeenCalledWith("task_detail", { id: "a" });
  });
});
