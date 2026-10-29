/**
 * Browser notifications (Plan4-U3, D6/S11): page-present `new
 * Notification` only — no service worker/push. Permission requests
 * MUST come from a user gesture; the helpers never fire a request
 * from an effect.
 */

export type NotifyPermission = "granted" | "denied" | "default" | "unsupported";

export function notifyPermission(): NotifyPermission {
  if (typeof Notification === "undefined") return "unsupported";
  return Notification.permission;
}

/** Call ONLY from a user gesture (button click). */
export async function requestNotifyPermission(): Promise<NotifyPermission> {
  if (typeof Notification === "undefined") return "unsupported";
  try {
    return await Notification.requestPermission();
  } catch {
    return Notification.permission;
  }
}

const ENABLED_KEY = "monitor.browserNotify.enabled";

export function browserNotifyEnabled(): boolean {
  try {
    return localStorage.getItem(ENABLED_KEY) === "1";
  } catch {
    return false;
  }
}

export function setBrowserNotifyEnabled(on: boolean): void {
  try {
    localStorage.setItem(ENABLED_KEY, on ? "1" : "0");
  } catch {
    /* storage unavailable — treat as off */
  }
}

export interface BrowserNotifyEvent {
  title: string;
  link?: string | null;
}

/**
 * Show a system notification for a stream event — only when the
 * permission is granted AND the user toggled the feature on.
 * `onNavigate` receives the deep link on click (focus is attempted
 * but may be denied by the browser — navigate regardless).
 */
export function maybeNotify(
  ev: BrowserNotifyEvent,
  onNavigate: (link: string) => void,
): boolean {
  if (!browserNotifyEnabled()) return false;
  if (notifyPermission() !== "granted") return false;
  try {
    const n = new Notification(ev.title, { tag: "ralph-monitor" });
    n.onclick = () => {
      try {
        window.focus();
      } catch {
        /* focus may be denied — navigate anyway */
      }
      if (ev.link) onNavigate(ev.link);
      n.close();
    };
    return true;
  } catch {
    return false;
  }
}
