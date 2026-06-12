/**
 * Pre-built workflow blueprints.
 *
 * Each blueprint is a complete hat collection with nodes, edges, and
 * metadata. When the user picks a blueprint, a new collection is created
 * with these nodes and edges pre-populated and auto-laid-out.
 */

import type { HatNodeData } from "./HatNode";

export interface Blueprint {
  id: string;
  name: string;
  description: string;
  emoji: string;
  hats: HatNodeData[];
  /** Edges as [sourceKey, targetKey, eventName] tuples. */
  edges: [string, string, string][];
}

export const BLUEPRINTS: Blueprint[] = [
  {
    id: "debug",
    name: "Debug",
    description: "Investigate → Test → Fix → Verify. For hunting bugs.",
    emoji: "🔍",
    hats: [
      {
        key: "investigator",
        name: "🔍 Investigator",
        description: "Reproduces the bug and identifies root cause",
        triggersOn: ["work.start", "fix.failed"],
        publishes: ["hypothesis.ready"],
      },
      {
        key: "fixer",
        name: "🔧 Fixer",
        description: "Applies the fix based on the hypothesis",
        triggersOn: ["hypothesis.ready"],
        publishes: ["fix.done", "fix.failed"],
      },
      {
        key: "verifier",
        name: "✅ Verifier",
        description: "Runs tests to confirm the fix works",
        triggersOn: ["fix.done"],
        publishes: ["LOOP_COMPLETE", "fix.failed"],
      },
    ],
    edges: [
      ["investigator", "fixer", "hypothesis.ready"],
      ["fixer", "verifier", "fix.done"],
      ["fixer", "investigator", "fix.failed"],
      ["verifier", "investigator", "fix.failed"],
    ],
  },
];
