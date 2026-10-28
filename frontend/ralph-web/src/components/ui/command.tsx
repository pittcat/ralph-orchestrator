/**
 * Command — self-implemented combobox primitives (plan U4 §6 chose
 * self-implementation over `cmdk`; focus management is a DOM-driven
 * roving `data-active` attribute, well under the 150-line budget).
 *
 * Keyboard contract: ↑/↓ moves the active option, Enter activates it
 * (dispatched as a real click so `onSelect`/`onClick` both work), Esc
 * and focus restoration are owned by the wrapping Dialog.
 */

import * as React from "react";
import { Search } from "lucide-react";
import { cn } from "@/lib/utils";
import { Dialog, DialogContent } from "./dialog";

interface CommandContextValue {
  query: string;
  setQuery: (q: string) => void;
}

const CommandContext = React.createContext<CommandContextValue>({
  query: "",
  setQuery: () => {},
});

const ITEM_SELECTOR = "[data-command-item]";

/** Root: owns the filter query + roving active option. */
function Command({ className, children, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  const [query, setQuery] = React.useState("");
  const [active, setActive] = React.useState(0);
  const listRef = React.useRef<HTMLDivElement>(null);

  const items = React.useCallback(
    () =>
      Array.from(
        listRef.current?.querySelectorAll(ITEM_SELECTOR) ?? [],
      ) as HTMLElement[],
    [],
  );

  // Keep `active` in range whenever the filtered item set changes.
  React.useLayoutEffect(() => {
    const els = items();
    if (els.length === 0) {
      if (active !== 0) setActive(0);
      return;
    }
    const clamped = Math.min(active, els.length - 1);
    if (clamped !== active) setActive(clamped);
    els.forEach((el, i) => {
      el.toggleAttribute("data-active", i === clamped);
      el.setAttribute("aria-selected", String(i === clamped));
    });
  });

  // Scroll the active option into view.
  React.useEffect(() => {
    items()[active]?.scrollIntoView({ block: "nearest" });
  }, [active, items]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const els = items();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, Math.max(els.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      els[active]?.click();
    }
  };

  return (
    <CommandContext.Provider value={{ query, setQuery }}>
      {/* eslint-disable-next-line jsx-a11y/no-static-element-interactions */}
      <div
        className={cn("flex flex-col overflow-hidden", className)}
        onKeyDown={onKeyDown}
        {...props}
      >
        {React.Children.map(children, (child) =>
          React.isValidElement(child) && child.type === CommandList
            ? React.cloneElement(child as React.ReactElement<{ listRef?: React.Ref<HTMLDivElement> }>, {
                listRef,
              })
            : child,
        )}
      </div>
    </CommandContext.Provider>
  );
}

function CommandInput({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  const { query, setQuery } = React.useContext(CommandContext);
  return (
    <div className="flex items-center gap-2 border-b border-border px-3">
      <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
      <input
        role="combobox"
        aria-expanded="true"
        aria-controls="command-list"
        autoFocus
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        className={cn(
          "flex h-11 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground",
          className,
        )}
        {...props}
      />
    </div>
  );
}

function CommandList({
  className,
  listRef,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & {
  listRef?: React.Ref<HTMLDivElement>;
}) {
  return (
    <div
      ref={listRef}
      id="command-list"
      role="listbox"
      className={cn("max-h-72 overflow-y-auto p-1", className)}
      {...props}
    />
  );
}

function CommandGroup({
  heading,
  className,
  children,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { heading?: string }) {
  return (
    <div className={cn("py-1", className)} role="group" aria-label={heading} {...props}>
      {heading && (
        <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
          {heading}
        </div>
      )}
      {children}
    </div>
  );
}

function CommandItem({
  className,
  children,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    // Interactive option — activated via Enter → click or direct click.
    // eslint-disable-next-line jsx-a11y/click-events-have-key-events
    <div
      role="option"
      tabIndex={-1}
      data-command-item
      className={cn(
        "flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none data-[active]:bg-accent data-[active]:text-accent-foreground",
        className,
      )}
      {...props}
    >
      {children}
    </div>
  );
}

function CommandEmpty({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn("py-6 text-center text-sm text-muted-foreground", className)}
      {...props}
    />
  );
}

/** Command rendered inside a Dialog; ⌘K / Ctrl+K toggles it open. */
function CommandDialog({
  open,
  onOpenChange,
  shortcutKey = "k",
  children,
  ...props
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Keyboard shortcut letter combined with Ctrl/Cmd; `null` disables. */
  shortcutKey?: string | null;
  children: React.ReactNode;
} & Omit<React.ComponentProps<typeof DialogContent>, "children">) {
  // Track the element focused before open so closing restores it —
  // Radix FocusScope does this in browsers but jsdom coverage of the
  // restore path is unreliable, so we make it explicit.
  const lastFocused = React.useRef<HTMLElement | null>(null);

  React.useEffect(() => {
    if (!shortcutKey) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === shortcutKey) {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onOpenChange, shortcutKey]);

  React.useEffect(() => {
    if (open) {
      lastFocused.current = document.activeElement as HTMLElement | null;
    } else {
      lastFocused.current?.focus();
      lastFocused.current = null;
    }
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="overflow-hidden p-0"
        onOpenAutoFocus={(e) => {
          // Focus the input, not the close button.
          e.preventDefault();
          (e.currentTarget as HTMLElement | null)
            ?.querySelector("input")
            ?.focus();
        }}
        {...props}
      >
        {children}
      </DialogContent>
    </Dialog>
  );
}

export {
  Command,
  CommandDialog,
  CommandInput,
  CommandList,
  CommandGroup,
  CommandItem,
  CommandEmpty,
};
