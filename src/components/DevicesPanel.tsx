// DevicesPanel
//
// Devices view for the operator UI.
//
// Features:
//   - Table of registered devices: Device ID, Hostname, Last Seen, Status, Actions
//   - Status pills: active (green), disabled (gray), revoked (red)
//   - Disable button gated on CmtraceOpen.Admin role via RoleGate
//   - Confirm modal before posting to POST /v1/admin/devices/{id}/disable
//   - Automatic refresh every 30 seconds + manual Refresh button
//   - Keyset pagination using nextCursor from the server
//   - Substring filter on hostname or device ID (client-side)
//   - Empty state: "No devices have checked in yet."

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { apiBase, disableDevice, listDevicesPage } from "../lib/api-client";
import { ROLE_ADMIN } from "../lib/auth-config";
import type { DeviceSummary, DeviceStatus } from "../lib/log-types";
import { useDebounce } from "../lib/use-debounce";
import { RoleGate } from "./RoleGate";

const PAGE_SIZE = 50;
const POLL_INTERVAL_MS = 30_000;
const FILTER_DEBOUNCE_MS = 250;
const ROW_HEIGHT_PX = 44;

type FetchState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: string };

export function DevicesPanel() {
  // All pages accumulated so far (we load page-by-page on "Load more").
  const [devices, setDevices] = useState<DeviceSummary[]>([]);
  const [fetchState, setFetchState] = useState<FetchState<null>>({ status: "idle" });
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [filter, setFilter] = useState("");

  // Per-row optimistic status overrides applied after a successful disable.
  const [disabledIds, setDisabledIds] = useState<Set<string>>(new Set());

  // Confirm modal state.
  const [confirmDevice, setConfirmDevice] = useState<DeviceSummary | null>(null);
  const [disabling, setDisabling] = useState(false);
  const [disableError, setDisableError] = useState<string | null>(null);

  const loadFirstPage = useCallback(async () => {
    setFetchState({ status: "loading" });
    try {
      const page = await listDevicesPage(PAGE_SIZE, null);
      setDevices(page.items);
      setNextCursor(page.nextCursor);
      setHasMore(page.nextCursor !== null);
      setFetchState({ status: "ok", data: null });
    } catch (err) {
      setFetchState({ status: "error", error: formatError(err) });
    }
  }, []);

  const loadNextPage = useCallback(async () => {
    if (!nextCursor) return;
    setFetchState({ status: "loading" });
    try {
      const page = await listDevicesPage(PAGE_SIZE, nextCursor);
      setDevices((prev) => [...prev, ...page.items]);
      setNextCursor(page.nextCursor);
      setHasMore(page.nextCursor !== null);
      setFetchState({ status: "ok", data: null });
    } catch (err) {
      setFetchState({ status: "error", error: formatError(err) });
    }
  }, [nextCursor]);

  // Initial load.
  useEffect(() => {
    void loadFirstPage();
  }, [loadFirstPage]);

  // Polling refresh — reloads from the first page every 30 s.
  // Paused while the confirm modal is open so the row the operator is
  // looking at can't be swapped out from under them mid-confirmation
  // (see review feedback: status pill flicker active → disabled → active).
  const loadFirstPageRef = useRef(loadFirstPage);
  loadFirstPageRef.current = loadFirstPage;
  useEffect(() => {
    if (confirmDevice !== null) return undefined;
    const id = setInterval(() => {
      void loadFirstPageRef.current();
    }, POLL_INTERVAL_MS);
    return () => clearInterval(id);
  }, [confirmDevice]);

  const handleDisableClick = useCallback((device: DeviceSummary) => {
    setConfirmDevice(device);
    setDisableError(null);
  }, []);

  const handleConfirmDisable = useCallback(async () => {
    if (!confirmDevice) return;
    setDisabling(true);
    setDisableError(null);
    try {
      await disableDevice(confirmDevice.deviceId);
      // Optimistically mark the row disabled without waiting for a full
      // list refresh — the polling will catch any server-side status later.
      setDisabledIds((prev) => new Set([...prev, confirmDevice.deviceId]));
      setConfirmDevice(null);
    } catch (err) {
      setDisableError(formatError(err));
    } finally {
      setDisabling(false);
    }
  }, [confirmDevice]);

  const handleCancelDisable = useCallback(() => {
    setConfirmDevice(null);
    setDisableError(null);
  }, []);

  // Client-side filter: substring match on deviceId or hostname.
  // Debounced so a flurry of keystrokes only triggers one re-filter pass.
  // 250 ms is the standard for filter UIs — fast enough to feel live, slow
  // enough that "rapid typing" coalesces into a single evaluation.
  const debouncedFilter = useDebounce(filter, FILTER_DEBOUNCE_MS);
  const filterLower = debouncedFilter.toLowerCase().trim();
  const filtered = useMemo(
    () =>
      filterLower === ""
        ? devices
        : devices.filter(
            (d) =>
              d.deviceId.toLowerCase().includes(filterLower) ||
              (d.hostname ?? "").toLowerCase().includes(filterLower),
          ),
    [devices, filterLower],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
        border: "1px solid #ddd",
        borderRadius: 4,
        overflow: "hidden",
        background: "white",
      }}
    >
      <PanelHeader
        total={devices.length}
        filtered={filtered.length}
        filter={filter}
        onFilterChange={setFilter}
        onRefresh={loadFirstPage}
        loading={fetchState.status === "loading"}
      />
      <div style={{ flex: 1, minHeight: 0, overflow: "auto" }}>
        {fetchState.status === "error" && (
          <ApiError error={fetchState.error} />
        )}
        {fetchState.status === "loading" && devices.length === 0 && (
          <CenteredText text="Loading devices…" muted />
        )}
        {fetchState.status === "ok" && devices.length === 0 && (
          <EmptyState />
        )}
        {devices.length > 0 && filterLower !== "" && filtered.length === 0 && (
          <CenteredText
            text={`No devices match "${debouncedFilter.trim()}".`}
            muted
          />
        )}
        {devices.length > 0 && filtered.length > 0 && (
          <DeviceTable
            devices={filtered}
            disabledIds={disabledIds}
            onDisable={handleDisableClick}
          />
        )}
        {hasMore && (
          <div style={{ padding: "8px 12px" }}>
            <button
              type="button"
              onClick={() => void loadNextPage()}
              disabled={fetchState.status === "loading"}
              style={secondaryBtn}
            >
              {fetchState.status === "loading" ? "Loading…" : "Load more"}
            </button>
          </div>
        )}
      </div>
      {confirmDevice && (
        <ConfirmModal
          device={confirmDevice}
          busy={disabling}
          error={disableError}
          onConfirm={() => void handleConfirmDisable()}
          onCancel={handleCancelDisable}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sub-components

function PanelHeader({
  total,
  filtered,
  filter,
  onFilterChange,
  onRefresh,
  loading,
}: {
  total: number;
  filtered: number;
  filter: string;
  onFilterChange: (v: string) => void;
  onRefresh: () => void;
  loading: boolean;
}) {
  return (
    <header
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "8px 12px",
        background: "#f5f5f5",
        borderBottom: "1px solid #ddd",
        flexWrap: "wrap",
      }}
    >
      <span style={{ fontWeight: 600, fontSize: 13, color: "#333" }}>
        Devices
      </span>
      {total > 0 && (
        <span style={{ fontSize: 11, color: "#888" }}>
          {filtered === total
            ? `${total} device${total === 1 ? "" : "s"}`
            : `${filtered} / ${total}`}
        </span>
      )}
      <span style={{ flex: 1 }} />
      <input
        type="search"
        placeholder="Filter by hostname or device ID…"
        value={filter}
        onChange={(e) => onFilterChange(e.target.value)}
        style={{
          padding: "4px 8px",
          fontSize: 12,
          border: "1px solid #ccc",
          borderRadius: 4,
          width: 240,
          color: "#222",
          background: "white",
        }}
      />
      <button
        type="button"
        onClick={onRefresh}
        disabled={loading}
        title="Refresh now"
        style={secondaryBtn}
      >
        {loading ? "Refreshing…" : "↻ Refresh"}
      </button>
      <span style={{ fontSize: 11, color: "#aaa" }}>
        base: {apiBase || "(same-origin)"}
      </span>
    </header>
  );
}

/**
 * Virtualized device table. Uses CSS-grid rows inside a scroll container so
 * `@tanstack/react-virtual`'s absolute-positioned rows align cleanly. Header
 * is a sticky grid row (matching the same column template as each body row);
 * body rows are virtualized via `useVirtualizer` keyed on `count: devices.length`.
 *
 * Row height is fixed (`ROW_HEIGHT_PX`) which keeps `estimateSize` exact, so
 * scroll position stays stable as the user filters.
 */
function DeviceTable({
  devices,
  disabledIds,
  onDisable,
}: {
  devices: DeviceSummary[];
  disabledIds: Set<string>;
  onDisable: (d: DeviceSummary) => void;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const rowVirtualizer = useVirtualizer({
    count: devices.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT_PX,
    overscan: 8,
  });

  const gridTemplate = "minmax(180px, 2fr) minmax(140px, 2fr) 180px 110px 110px";

  return (
    <div
      ref={scrollRef}
      style={{
        height: "100%",
        overflow: "auto",
        fontSize: 13,
        position: "relative",
      }}
      data-testid="device-table-scroll"
    >
      {/* Sticky header row */}
      <div
        role="row"
        style={{
          display: "grid",
          gridTemplateColumns: gridTemplate,
          background: "#fafafa",
          borderBottom: "1px solid #e5e5e5",
          position: "sticky",
          top: 0,
          zIndex: 1,
        }}
      >
        <HeaderCell>Device ID</HeaderCell>
        <HeaderCell>Hostname</HeaderCell>
        <HeaderCell>Last Seen</HeaderCell>
        <HeaderCell>Status</HeaderCell>
        <HeaderCell>Actions</HeaderCell>
      </div>
      {/* Virtualized body */}
      <div
        style={{
          height: `${rowVirtualizer.getTotalSize()}px`,
          width: "100%",
          position: "relative",
        }}
      >
        {rowVirtualizer.getVirtualItems().map((virtualRow) => {
          const d = devices[virtualRow.index];
          if (!d) return null;
          const effectiveStatus: DeviceStatus = disabledIds.has(d.deviceId)
            ? "disabled"
            : (d.status ?? "active");
          return (
            <div
              key={d.deviceId}
              role="row"
              data-testid="device-row"
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: `${virtualRow.size}px`,
                transform: `translateY(${virtualRow.start}px)`,
                display: "grid",
                gridTemplateColumns: gridTemplate,
                borderBottom: "1px solid #f0f0f0",
                alignItems: "center",
              }}
            >
              <BodyCell>
                <span
                  title={d.deviceId}
                  style={{
                    fontFamily: "ui-monospace, Menlo, Consolas, monospace",
                    fontSize: 12,
                  }}
                >
                  {truncateDeviceId(d.deviceId)}
                </span>
              </BodyCell>
              <BodyCell>
                {d.hostname ?? <span style={{ color: "#aaa" }}>—</span>}
              </BodyCell>
              <BodyCell>{formatUtc(d.lastSeenUtc)}</BodyCell>
              <BodyCell>
                <StatusPill status={effectiveStatus} />
              </BodyCell>
              <BodyCell>
                <RoleGate
                  role={ROLE_ADMIN}
                  fallback={
                    <button type="button" disabled style={disabledActionBtn}>
                      Disable
                    </button>
                  }
                >
                  <button
                    type="button"
                    onClick={() => onDisable(d)}
                    disabled={effectiveStatus !== "active"}
                    style={
                      effectiveStatus !== "active"
                        ? disabledActionBtn
                        : dangerBtn
                    }
                    title={
                      effectiveStatus !== "active"
                        ? `Device is already ${effectiveStatus}`
                        : `Disable device ${d.deviceId}`
                    }
                  >
                    Disable
                  </button>
                </RoleGate>
              </BodyCell>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function HeaderCell({ children }: { children: React.ReactNode }) {
  return (
    <div
      role="columnheader"
      style={{
        padding: "8px 12px",
        textAlign: "left",
        fontWeight: 600,
        fontSize: 11,
        color: "#555",
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </div>
  );
}

function BodyCell({ children }: { children: React.ReactNode }) {
  return (
    <div
      role="cell"
      style={{
        padding: "7px 12px",
        fontSize: 13,
        color: "#222",
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </div>
  );
}

function StatusPill({ status }: { status: DeviceStatus }) {
  const styles: Record<DeviceStatus, React.CSSProperties> = {
    active: {
      background: "#dcfce7",
      color: "#15803d",
      border: "1px solid #86efac",
    },
    disabled: {
      background: "#f3f4f6",
      color: "#6b7280",
      border: "1px solid #d1d5db",
    },
    revoked: {
      background: "#fef2f2",
      color: "#b91c1c",
      border: "1px solid #fca5a5",
    },
  };
  const label: Record<DeviceStatus, string> = {
    active: "Active",
    disabled: "Disabled",
    revoked: "Revoked",
  };
  return (
    <span
      style={{
        display: "inline-block",
        padding: "2px 8px",
        borderRadius: 12,
        fontSize: 11,
        fontWeight: 600,
        ...styles[status],
      }}
    >
      {label[status]}
    </span>
  );
}

function ConfirmModal({
  device,
  busy,
  error,
  onConfirm,
  onCancel,
}: {
  device: DeviceSummary;
  busy: boolean;
  error: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  // Close on Escape.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [busy, onCancel]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "rgba(0,0,0,0.35)",
        zIndex: 100,
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget && !busy) onCancel();
      }}
    >
      <div
        style={{
          background: "white",
          borderRadius: 6,
          padding: 24,
          maxWidth: 440,
          width: "90%",
          boxShadow: "0 8px 32px rgba(0,0,0,0.18)",
        }}
      >
        <h3
          id="confirm-title"
          style={{ margin: "0 0 12px", fontSize: 15, fontWeight: 600 }}
        >
          Disable device?
        </h3>
        <p style={{ margin: "0 0 8px", fontSize: 13, color: "#444" }}>
          Device:{" "}
          <code
            style={{
              background: "#f3f4f6",
              padding: "1px 4px",
              borderRadius: 3,
              fontSize: 12,
              wordBreak: "break-all",
            }}
            data-testid="confirm-device-id"
          >
            {device.deviceId}
          </code>
        </p>
        <p style={{ margin: "0 0 8px", fontSize: 13, color: "#444" }}>
          This will post{" "}
          <code
            style={{
              background: "#f3f4f6",
              padding: "1px 4px",
              borderRadius: 3,
              fontSize: 12,
            }}
          >
            {`POST /v1/admin/devices/{id}/disable`}
          </code>{" "}
          using your operator bearer token.
        </p>
        <p style={{ margin: "0 0 16px", fontSize: 13, color: "#666" }}>
          The device will be unable to check in until re-enabled via Cloud
          PKI cert revocation. This action cannot be undone in the UI.
        </p>
        {error && (
          <div
            style={{
              marginBottom: 12,
              padding: "8px 10px",
              background: "#fef2f2",
              border: "1px solid #fecaca",
              color: "#991b1b",
              borderRadius: 4,
              fontSize: 12,
              whiteSpace: "pre-wrap",
            }}
          >
            {error}
          </div>
        )}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            style={secondaryBtn}
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={busy}
            style={dangerBtn}
          >
            {busy ? "Disabling…" : "Yes, disable"}
          </button>
        </div>
      </div>
    </div>
  );
}

function EmptyState() {
  return (
    <div
      style={{
        padding: 32,
        textAlign: "center",
        color: "#777",
        fontSize: 13,
      }}
    >
      No devices have checked in yet.
    </div>
  );
}

function ApiError({ error }: { error: string }) {
  const unreachable =
    /failed to fetch|networkerror|fetch failed/i.test(error);
  return (
    <div
      style={{
        margin: 10,
        padding: "10px 12px",
        background: "#fef2f2",
        border: "1px solid #fecaca",
        color: "#991b1b",
        borderRadius: 4,
        fontSize: 13,
        whiteSpace: "pre-wrap",
      }}
    >
      {unreachable
        ? `Cannot reach API at ${apiBase || "(same-origin)"}. Is the api-server running?`
        : error}
    </div>
  );
}

function CenteredText({ text, muted }: { text: string; muted?: boolean }) {
  return (
    <div style={{ padding: 14, color: muted ? "#777" : "#222", fontSize: 13 }}>
      {text}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Styles + helpers

const secondaryBtn: React.CSSProperties = {
  padding: "4px 10px",
  fontSize: 12,
  border: "1px solid #ccc",
  background: "white",
  borderRadius: 4,
  cursor: "pointer",
  color: "#222",
};

const dangerBtn: React.CSSProperties = {
  padding: "4px 10px",
  fontSize: 12,
  border: "1px solid #fca5a5",
  background: "#fef2f2",
  borderRadius: 4,
  cursor: "pointer",
  color: "#b91c1c",
  fontWeight: 500,
};

const disabledActionBtn: React.CSSProperties = {
  ...secondaryBtn,
  opacity: 0.45,
  cursor: "not-allowed",
};

/**
 * Shorten long SAN URI device IDs (e.g. "device:acme/prod-server-01") to a
 * display-friendly slug. Full value is available in the `title` attribute.
 */
function truncateDeviceId(id: string): string {
  if (id.length <= 36) return id;
  return `${id.slice(0, 16)}…${id.slice(-12)}`;
}

function formatUtc(iso: string): string {
  return iso.replace("T", " ").replace(/\.\d+Z?$/, "").replace(/Z$/, "");
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
