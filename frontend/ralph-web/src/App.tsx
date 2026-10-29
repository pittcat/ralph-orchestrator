/**
 * App Component
 *
 * Application routing configuration using React Router.
 * Defines routes with AppShell layout and page components.
 */

import { Routes, Route, Navigate } from "react-router-dom";
import { AppShell } from "./components/layout";
import { TasksPage, PlanPage, BuilderPage, TaskDetailPage, SettingsPage } from "./pages";
import { MonitorLayout } from "./features/monitor/MonitorLayout";
import { OverviewPage } from "./features/monitor/OverviewPage";
import { RunDetailPage } from "./features/monitor/RunDetailPage";
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
      {/* AppShell provides the layout, Outlet renders the matched route */}
      <Route element={<AppShell />}>
        <Route path="/tasks" element={<TasksPage />} />
        <Route path="/tasks/:id" element={<TaskDetailPage />} />
        <Route path="/builder" element={<BuilderPage />} />
        <Route path="/plan" element={<PlanPage />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route
          path="/monitor"
          element={
            <Monitor>
              <OverviewPage />
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
        {/* Redirect root to tasks */}
        <Route path="/" element={<Navigate to="/tasks" replace />} />
        {/* Catch-all redirect to tasks */}
        <Route path="*" element={<Navigate to="/tasks" replace />} />
      </Route>
    </Routes>
  );
}
