import type { UploadedDataset } from "./types";

interface ApiEnvelope<T> {
  success: boolean;
  data?: T;
  error?: string;
}

/**
 * Upload a CSV or JSON array-of-objects to the backend. The server
 * writes the file to a tempdir, infers schema + a 5-row sample, and
 * returns a dataset_id the caller can later bind to a chat session.
 */
export async function uploadDataset(file: File): Promise<UploadedDataset> {
  const fd = new FormData();
  fd.append("file", file);
  const res = await fetch("/api/datasets/upload", {
    method: "POST",
    body: fd,
  });
  const body = (await res.json()) as ApiEnvelope<UploadedDataset>;
  if (!res.ok || !body.success || !body.data) {
    throw new Error(body.error || `upload failed: ${res.status}`);
  }
  return body.data;
}

/**
 * Attach an uploaded dataset to a chat session. The backend will then
 * spawn the Python MCP sidecar on every Claude turn for that session
 * until the session is unbound.
 */
export async function bindDataset(
  sessionId: string,
  datasetId: string,
): Promise<void> {
  const res = await fetch("/api/datasets/bind", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ session_id: sessionId, dataset_id: datasetId }),
  });
  const body = (await res.json()) as ApiEnvelope<null>;
  if (!res.ok || !body.success) {
    throw new Error(body.error || `bind failed: ${res.status}`);
  }
}

/**
 * Drop a session's dataset binding. The underlying dataset is preserved
 * (the user can rebind it elsewhere); this just tells the backend to
 * stop attaching the Python MCP sidecar on subsequent turns.
 */
export async function unbindDataset(sessionId: string): Promise<void> {
  try {
    await fetch("/api/datasets/unbind", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ session_id: sessionId }),
    });
  } catch (e) {
    console.warn("[datasets] unbind failed", e);
  }
}
