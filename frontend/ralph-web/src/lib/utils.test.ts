import { afterEach, describe, expect, it, vi } from "vitest";
import { newIdempotencyKey } from "./utils";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("newIdempotencyKey", () => {
  it("uses crypto.randomUUID in secure contexts", () => {
    const key = newIdempotencyKey();
    expect(key).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    );
  });

  it("falls back when randomUUID is absent (insecure LAN origin)", () => {
    const realCrypto = globalThis.crypto;
    vi.stubGlobal("crypto", {
      getRandomValues: realCrypto.getRandomValues.bind(realCrypto),
    });
    const key = newIdempotencyKey();
    expect(key).toMatch(/^op-[0-9a-f]{32}$/);
  });

  it("falls back to Math.random when crypto is entirely absent", () => {
    vi.stubGlobal("crypto", undefined);
    expect(newIdempotencyKey()).toMatch(/^op-.+/);
  });
});
