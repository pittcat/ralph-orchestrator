/**
 * toast.test.tsx — queue, auto-dismiss, manual dismiss.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast, Toaster } from "./toast";

describe("Toaster", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  it("shows a toast and auto-dismisses after the duration", async () => {
    render(<Toaster />);
    toast("hello monitor", { durationMs: 100 });
    expect(await screen.findByText("hello monitor")).toBeInTheDocument();
    vi.advanceTimersByTime(200);
    await waitFor(() =>
      expect(screen.queryByText("hello monitor")).not.toBeInTheDocument(),
    );
  });

  it("manual dismiss button removes the toast", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    render(<Toaster />);
    toast("dismiss me", { durationMs: 60000 });
    await user.click(await screen.findByLabelText("Dismiss"));
    await waitFor(() =>
      expect(screen.queryByText("dismiss me")).not.toBeInTheDocument(),
    );
  });
});
