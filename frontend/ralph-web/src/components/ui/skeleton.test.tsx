/**
 * skeleton.test.tsx — aria-busy placeholder.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { Skeleton } from "./skeleton";

describe("Skeleton", () => {
  it("renders an aria-busy block", () => {
    const { container } = render(<Skeleton className="h-4 w-4" />);
    expect(container.firstChild).toHaveAttribute("aria-busy", "true");
  });
});
