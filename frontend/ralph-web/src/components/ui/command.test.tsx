/**
 * command.test.tsx — ⌘K open/close, filter, Enter activation, focus return.
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "./command";

function Palette({ onSelect }: { onSelect: (v: string) => void }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const items = ["alpha", "beta", "gamma"].filter((i) => i.includes(query));
  return (
    <>
      <button data-testid="trigger">open</button>
      <CommandDialog open={open} onOpenChange={setOpen}>
        <Command>
          <CommandInput
            placeholder="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <CommandList>
            <CommandEmpty>none</CommandEmpty>
            {items.map((i) => (
              <CommandItem
                key={i}
                onClick={() => {
                  onSelect(i);
                  setOpen(false);
                  setQuery("");
                }}
              >
                {i}
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      </CommandDialog>
    </>
  );
}

describe("CommandDialog", () => {
  it("Ctrl+K opens the palette and focuses the input", async () => {
    const user = userEvent.setup();
    render(<Palette onSelect={() => {}} />);
    await user.keyboard("{Control>}k{/Control}");
    expect(await screen.findByPlaceholderText("search")).toHaveFocus();
  });

  it("Esc closes and returns focus to the previously focused element", async () => {
    const user = userEvent.setup();
    render(<Palette onSelect={() => {}} />);
    const trigger = screen.getByTestId("trigger");
    await user.click(trigger);
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByPlaceholderText("search");
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByPlaceholderText("search")).not.toBeInTheDocument(),
    );
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("typing filters items; Enter activates the active option", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Palette onSelect={onSelect} />);
    await user.keyboard("{Control>}k{/Control}");
    const input = await screen.findByPlaceholderText("search");
    await user.type(input, "gam");
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith("gamma");
  });

  it("ArrowDown/ArrowUp move the active option before Enter", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Palette onSelect={onSelect} />);
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByPlaceholderText("search");
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(onSelect).toHaveBeenCalledWith("gamma");
    // reopen — ArrowUp from top clamps at 0
    onSelect.mockClear();
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByPlaceholderText("search");
    await user.keyboard("{ArrowUp}{Enter}");
    expect(onSelect).toHaveBeenCalledWith("alpha");
  });
});
