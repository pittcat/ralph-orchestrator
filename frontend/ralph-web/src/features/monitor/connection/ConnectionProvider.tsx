/**
 * ConnectionProvider (Plan3-U4): owns the single monitor stream for
 * the whole feature. Children get data through react-query (the
 * stream only invalidates); the context exposes the connection badge
 * state. StrictMode double-mount is safe — the effect cleanup stops
 * the driver so exactly one live stream exists per mounted tree.
 */

import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  startMonitorStream,
  type MonitorStreamState,
  type StreamDriver,
} from "./useMonitorStream";
import { maybeNotify } from "../browserNotify";
import { useNavigate } from "react-router-dom";

const ConnectionCtx = createContext<MonitorStreamState>({
  status: "connecting",
  lastSeq: 0,
  lastValidAt: null,
  attempts: 0,
});

export function useMonitorConnection(): MonitorStreamState {
  return useContext(ConnectionCtx);
}

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [state, setState] = useState<MonitorStreamState>({
    status: "connecting",
    lastSeq: 0,
    lastValidAt: null,
    attempts: 0,
  });

  useEffect(() => {
    const driver: StreamDriver = startMonitorStream(qc, {
      onChange: setState,
      onEvent: ({ kind, data }) => {
        // Plan4-U3 S11: stream → system notification (page-present only).
        if (kind === "notification.created") {
          const p = data as { title?: string; link?: string | null } | undefined;
          if (p?.title) {
            maybeNotify({ title: p.title, link: p.link ?? null }, navigate);
          }
        }
      },
    });
    return () => driver.stop();
  }, [qc, navigate]);

  return <ConnectionCtx.Provider value={state}>{children}</ConnectionCtx.Provider>;
}
