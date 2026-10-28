/**
 * Monitor command palette — ⌘K/Ctrl+K, searches real projects and
 * recent runs via the U3 API and navigates on select.
 */

import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { FolderGit2, Play } from "lucide-react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { useMonitorProjects, useMonitorRuns } from "./hooks";

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const projects = useMonitorProjects();
  const runs = useMonitorRuns({ limit: 10 });
  const [query, setQuery] = useState("");

  const q = query.toLowerCase();
  const filteredProjects = (projects.data ?? []).filter(
    (p) =>
      p.displayName.toLowerCase().includes(q) ||
      p.canonicalPath.toLowerCase().includes(q),
  );
  const filteredRuns = (runs.data?.runs ?? []).filter(
    (r) =>
      r.runId.toLowerCase().includes(q) ||
      (r.promptPreview ?? "").toLowerCase().includes(q) ||
      (r.preset ?? "").toLowerCase().includes(q),
  );

  const go = (to: string) => {
    setOpen(false);
    setQuery("");
    navigate(to);
  };

  return (
    <CommandDialog open={open} onOpenChange={setOpen} aria-label="Command palette">
      <Command>
        <CommandInput
          placeholder="Search projects and runs…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <CommandList>
          <CommandEmpty>No results.</CommandEmpty>
          {filteredProjects.length > 0 && (
            <CommandGroup heading="Projects">
              {filteredProjects.map((p) => (
                <CommandItem
                  key={p.id}
                  onClick={() => go(`/monitor?project=${p.id}`)}
                >
                  <FolderGit2 className="h-4 w-4 text-muted-foreground" />
                  <span className="truncate">{p.displayName}</span>
                  <span className="ml-auto truncate text-xs text-muted-foreground">
                    {p.canonicalPath}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          )}
          {filteredRuns.length > 0 && (
            <CommandGroup heading="Recent runs">
              {filteredRuns.map((r) => (
                <CommandItem
                  key={r.globalRunId}
                  onClick={() => go(`/monitor/runs/${encodeURIComponent(r.globalRunId)}`)}
                >
                  <Play className="h-4 w-4 text-muted-foreground" />
                  <span className="truncate font-mono text-xs">{r.runId}</span>
                  <span className="ml-auto text-xs text-muted-foreground">
                    {r.state}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          )}
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
