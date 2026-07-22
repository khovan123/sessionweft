export const CLIENT_PROTOCOL_VERSION = 1;

export interface ApiEnvelope<T> {
  protocol_version: number;
  correlation_id: string;
  data: T;
}

export interface RuntimeErrorEnvelope {
  protocol_version: number;
  correlation_id: string;
  code: string;
  message: string;
  retryable: boolean;
  committed_version?: number;
}

export interface PendingApproval {
  workflow_id: string;
  node_id: string;
  expected_version: number;
  title: string;
  reason?: string;
}

export interface ClientResourceView {
  protocol_version: number;
  session_id: string;
  session: unknown;
  agent?: unknown;
  workflow?: unknown;
  locks: unknown[];
  pending_approvals: PendingApproval[];
  generated_at: string;
}

export interface ClientEventRecord {
  cursor: number;
  envelope: {
    event_id: string;
    event_type: string;
    session_id?: string;
    occurred_at: string;
    payload: unknown;
  };
}

export interface EventBatch {
  protocol_version: number;
  after: number;
  next: number;
  latest: number;
  events: ClientEventRecord[];
  has_more: boolean;
}

export class RuntimeClient {
  constructor(
    private readonly endpoint: string,
    private readonly token?: string,
  ) {}

  async clientView(
    sessionId: string,
    options: { agentId?: string; workflowId?: string; workspaceId?: string },
    signal?: AbortSignal,
  ): Promise<ClientResourceView> {
    const query = new URLSearchParams();
    if (options.agentId) query.set("agent_id", options.agentId);
    if (options.workflowId) query.set("workflow_id", options.workflowId);
    if (options.workspaceId) query.set("workspace_id", options.workspaceId);
    const suffix = query.size > 0 ? `?${query.toString()}` : "";
    const envelope = await this.request<ApiEnvelope<ClientResourceView>>(
      `/v1/sessions/${encodeURIComponent(sessionId)}/client-view${suffix}`,
      { signal },
    );
    this.assertProtocol(envelope.protocol_version);
    return envelope.data;
  }

  async events(after: number, limit = 100, signal?: AbortSignal): Promise<EventBatch> {
    const envelope = await this.request<ApiEnvelope<EventBatch>>(
      `/v1/events?after=${after}&limit=${limit}`,
      { signal },
    );
    this.assertProtocol(envelope.protocol_version);
    return envelope.data;
  }

  async decideApproval(
    sessionId: string,
    approval: PendingApproval,
    approved: boolean,
    signal?: AbortSignal,
  ): Promise<void> {
    await this.request(
      `/v1/sessions/${encodeURIComponent(sessionId)}/workflows/${encodeURIComponent(
        approval.workflow_id,
      )}/nodes/${encodeURIComponent(approval.node_id)}/approval`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          expected_version: approval.expected_version,
          approved,
        }),
        signal,
      },
    );
  }

  private async request<T = unknown>(path: string, init: RequestInit = {}): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("accept", "application/json");
    if (this.token) headers.set("authorization", `Bearer ${this.token}`);
    const response = await fetch(`${this.endpoint.replace(/\/$/, "")}${path}`, {
      ...init,
      headers,
    });
    const text = await response.text();
    const payload: unknown = text.length > 0 ? JSON.parse(text) : undefined;
    if (!response.ok) {
      const runtimeError = payload as Partial<RuntimeErrorEnvelope> | undefined;
      const message = runtimeError?.message ?? `Runtime returned HTTP ${response.status}`;
      throw new Error(message);
    }
    return payload as T;
  }

  private assertProtocol(version: number): void {
    if (version !== CLIENT_PROTOCOL_VERSION) {
      throw new Error(
        `Unsupported Runtime protocol ${version}; extension supports ${CLIENT_PROTOCOL_VERSION}`,
      );
    }
  }
}
