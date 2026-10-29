/**
 * App Component
 *
 * Application routing — the dashboard is monitor-only: every route
 * resolves to a monitor page or redirects to the monitor overview.
 */

import { Routes, Route, Navigate } from "react-router-dom";
import { MonitorLayout } from "./features/monitor/MonitorLayout";
import { OverviewPage } from "./features/monitor/OverviewPage";
import { RunDetailPage } from "./features/monitor/RunDetailPage";
import { InboxPage } from "./features/monitor/inbox/InboxPage";
import { OperationsPage } from "./features/monitor/ops/OperationsPage";
import { useThemeEffect } from "./features/monitor/theme";
import { ConnectionProvider } from "./features/monitor/connection/ConnectionProvider";
import type { ReactNode } from "react";

/** Monitor pages share one stream lifetime per mounted subtree. */
function Monitor({ children }: { children: ReactNode }) {
  return (
    <ConnectionProvider>
      <MonitorLayout>{children}</MonitorLayout>
    </ConnectionProvider>
  );
}

export function App() {
  useThemeEffect();

  return (
    <Routes>
      <Route
        path="/monitor"
        element={
          <Monitor>
            <OverviewPage />
          </Monitor>
        }
      />
      <Route
        path="/monitor/inbox"
        element={
          <Monitor>
            <InboxPage />
          </Monitor>
        }
      />
      <Route
        path="/monitor/operations"
        element={
          <Monitor>
            <OperationsPage />
          </Monitor>
        }
      />
      <Route
        path="/monitor/runs/:globalId"
        element={
          <Monitor>
            <RunDetailPage />
          </Monitor>
        }
      />
      {/* Everything else lands on the monitor overview. */}
      <Route path="/" element={<Navigate to="/monitor" replace />} />
      <Route path="*" element={<Navigate to="/monitor" replace />} />
    </Routes>
  );
}
