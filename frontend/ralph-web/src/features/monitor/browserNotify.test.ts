/**
 * browserNotify (Plan4-U3): permission branch matrix — unsupported /
 * denied / granted+off / granted+on / default; toggle persistence;
 * notification click → navigate.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  browserNotifyEnabled,
  maybeNotify,
  notifyPermission,
  requestNotifyPermission,
  setBrowserNotifyEnabled,
} from "./browserNotify";

const orig = globalThis.Notification;

function stubNotification(
  permission: string,
  requestResult?: string,
): { instances: Array<{ onclick: (() => void) | null; close: () => void }> } {
  const instances: Array<{ onclick: (() => void) | null; close: () => void }> = [];
  const Ctor = class {
    static permission = permission;
    static async requestPermission() {
      Ctor.permission = requestResult ?? permission;
      return Ctor.permission;
    }
    onclick: (() => void) | null = null;
    close = vi.fn();
    constructor(public title: string) {
      instances.push(this);
    }
  };
  vi.stubGlobal("Notification", Ctor);
  return { instances };
}

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  if (orig === undefined) {
    // @ts-expect-error cleanup
    delete globalThis.Notification;
  } else {
    vi.stubGlobal("Notification", orig);
  }
  vi.unstubAllGlobals();
});

describe("permission matrix", () => {
  it("unsupported when Notification undefined", () => {
    // @ts-expect-error remove global
    delete globalThis.Notification;
    expect(notifyPermission()).toBe("unsupported");
    expect(maybeNotify({ title: "x" }, vi.fn())).toBe(false);
  });

  it("denied → disabled, no notification", () => {
    stubNotification("denied");
    setBrowserNotifyEnabled(true);
    const nav = vi.fn();
    expect(notifyPermission()).toBe("denied");
    expect(maybeNotify({ title: "x" }, nav)).toBe(false);
  });

  it("granted but toggle off → no notification", () => {
    stubNotification("granted");
    expect(maybeNotify({ title: "x" }, vi.fn())).toBe(false);
  });

  it("granted + enabled → shows notification, click navigates", () => {
    const { instances } = stubNotification("granted");
    setBrowserNotifyEnabled(true);
    const nav = vi.fn();
    expect(maybeNotify({ title: "run failed", link: "/monitor/inbox" }, nav)).toBe(true);
    expect(instances).toHaveLength(1);
    instances[0].onclick?.();
    expect(nav).toHaveBeenCalledWith("/monitor/inbox");
    expect(instances[0].close).toHaveBeenCalled();
  });

  it("default → requestPermission resolves and updates state", async () => {
    stubNotification("default", "granted");
    expect(notifyPermission()).toBe("default");
    await expect(requestNotifyPermission()).resolves.toBe("granted");
    expect(notifyPermission()).toBe("granted");
  });

  it("toggle persists across reload", () => {
    expect(browserNotifyEnabled()).toBe(false);
    setBrowserNotifyEnabled(true);
    expect(browserNotifyEnabled()).toBe(true);
  });
});
