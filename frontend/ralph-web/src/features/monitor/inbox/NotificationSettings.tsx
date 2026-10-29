/**
 * NotificationSettings (Plan4-U3, S10/S11/S13): webhook channel form
 * (masked secret echo, save, real test delivery result) + browser
 * notification toggle (permission state machine — the request only
 * ever fires from the enable button's click gesture) + the runtime
 * webhook duplicate hint.
 */

import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  apiPutChannelConfig,
  apiTestWebhook,
  type ChannelConfig,
} from "../api";
import {
  browserNotifyEnabled,
  notifyPermission,
  requestNotifyPermission,
  setBrowserNotifyEnabled,
  type NotifyPermission,
} from "../browserNotify";

export function NotificationSettings({
  config,
  runtimeWebhookHints,
}: {
  config: ChannelConfig | undefined;
  /** project display names whose ralph.yml also fires (S13). */
  runtimeWebhookHints: string[];
}) {
  const qc = useQueryClient();
  const [url, setUrl] = useState(config?.url ?? "");
  const [secret, setSecret] = useState("");
  const [enabled, setEnabled] = useState(config?.enabled ?? false);
  const [status, setStatus] = useState<string | null>(null);
  const [perm, setPerm] = useState<NotifyPermission>(notifyPermission());
  const [browserOn, setBrowserOn] = useState(browserNotifyEnabled());

  const save = async () => {
    setStatus(null);
    try {
      const saved = await apiPutChannelConfig({
        url,
        ...(secret ? { secret } : {}),
        enabled,
      });
      setSecret("");
      setStatus("已保存");
      void qc.invalidateQueries({ queryKey: ["monitor", "channel-config"] });
      void saved;
    } catch (e) {
      setStatus(`保存失败: ${e instanceof Error ? e.message : e}`);
    }
  };

  const test = async () => {
    setStatus("投递中…");
    try {
      const r = await apiTestWebhook(url || undefined);
      setStatus(
        r.outcome === "delivered"
          ? `测试投递成功 (delivery ${r.delivery_id.slice(0, 8)}…)`
          : `测试投递失败: ${r.error ?? r.outcome}`,
      );
    } catch (e) {
      setStatus(`测试投递失败: ${e instanceof Error ? e.message : e}`);
    }
  };

  // Permission request must come from this click — never an effect.
  const toggleBrowser = async () => {
    if (!browserOn && perm === "default") {
      const p = await requestNotifyPermission();
      setPerm(p);
      if (p !== "granted") return;
    }
    if (!browserOn && perm !== "granted") return;
    const next = !browserOn;
    setBrowserOn(next);
    setBrowserNotifyEnabled(next);
  };

  return (
    <section data-testid="notification-settings" className="space-y-4">
      <h3 className="text-sm font-semibold">通知设置</h3>

      {runtimeWebhookHints.length > 0 && (
        <p
          data-testid="runtime-webhook-hint"
          className="rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-700 dark:text-amber-300"
        >
          项目 {runtimeWebhookHints.join("、")} 还配置了 runtime 终态
          webhook，可能产生重复通知
        </p>
      )}

      <div className="space-y-2 rounded-md border border-border p-3">
        <h4 className="text-xs font-semibold text-muted-foreground">
          Webhook 渠道
        </h4>
        <label className="block text-xs">
          URL
          <input
            data-testid="webhook-url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://example.com/hook"
            className="mt-1 w-full rounded border border-border bg-background px-2 py-1 text-sm"
          />
        </label>
        <label className="block text-xs">
          Secret
          <input
            data-testid="webhook-secret"
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={config?.secret ?? "留空保持不变"}
            className="mt-1 w-full rounded border border-border bg-background px-2 py-1 text-sm"
          />
        </label>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            data-testid="webhook-enabled"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          启用 webhook 投递
        </label>
        <div className="flex gap-2">
          <button
            type="button"
            data-testid="webhook-save"
            onClick={() => void save()}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            保存
          </button>
          <button
            type="button"
            data-testid="webhook-test"
            onClick={() => void test()}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            发送测试通知
          </button>
        </div>
        {config?.last_error && (
          <p className="text-xs text-red-600">last error: {config.last_error}</p>
        )}
        {status && (
          <p data-testid="settings-status" className="text-xs text-muted-foreground">
            {status}
          </p>
        )}
      </div>

      <div className="space-y-2 rounded-md border border-border p-3">
        <h4 className="text-xs font-semibold text-muted-foreground">
          浏览器通知
        </h4>
        {perm === "unsupported" && (
          <p className="text-xs text-muted-foreground">
            当前环境不支持浏览器通知
          </p>
        )}
        {perm === "denied" && (
          <p data-testid="perm-denied" className="text-xs text-muted-foreground">
            通知权限已被拒绝——请在浏览器站点设置中重新允许
          </p>
        )}
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            data-testid="browser-notify-toggle"
            checked={browserOn}
            disabled={perm !== "granted" && perm !== "default"}
            onChange={() => void toggleBrowser()}
          />
          页面打开时显示系统通知
        </label>
      </div>
    </section>
  );
}
