/**
 * dialog.test.tsx — open/Esc close/overlay content.
 */

import { describe, it, expect } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "./dialog";

function Demo() {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <button>launch</button>
      </DialogTrigger>
      <DialogContent>
        <DialogTitle>Title</DialogTitle>
        <DialogDescription>desc</DialogDescription>
      </DialogContent>
    </Dialog>
  );
}

describe("Dialog", () => {
  it("opens on trigger, renders title/description", async () => {
    const user = userEvent.setup();
    render(<Demo />);
    await user.click(screen.getByText("launch"));
    expect(await screen.findByText("Title")).toBeInTheDocument();
    expect(screen.getByText("desc")).toBeInTheDocument();
  });

  it("Esc closes the dialog", async () => {
    const user = userEvent.setup();
    render(<Demo />);
    await user.click(screen.getByText("launch"));
    await screen.findByText("Title");
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByText("Title")).not.toBeInTheDocument(),
    );
  });

  it("close button dismisses", async () => {
    const user = userEvent.setup();
    render(<Demo />);
    await user.click(screen.getByText("launch"));
    await user.click(await screen.findByLabelText("Close"));
    await waitFor(() =>
      expect(screen.queryByText("Title")).not.toBeInTheDocument(),
    );
  });
});
