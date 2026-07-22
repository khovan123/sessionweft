import * as vscode from "vscode";
import {
  ClientEventRecord,
  ClientResourceView,
  PendingApproval,
  RuntimeClient,
} from "./protocol";

const TOKEN_SECRET = "sessionweft.runtimeToken";
const SESSION_KEY = "sessionweft.sessionId";
const CURSOR_KEY = "sessionweft.eventCursor";

class RuntimeNode extends vscode.TreeItem {
  constructor(
    label: string,
    collapsibleState: vscode.TreeItemCollapsibleState,
    public readonly children: RuntimeNode[] = [],
    description?: string,
  ) {
    super(label, collapsibleState);
    this.description = description;
  }
}

class RuntimeTreeProvider implements vscode.TreeDataProvider<RuntimeNode>, vscode.Disposable {
  private readonly changed = new vscode.EventEmitter<RuntimeNode | undefined | void>();
  readonly onDidChangeTreeData = this.changed.event;
  private view?: ClientResourceView;
  private events: ClientEventRecord[] = [];
  private status = "Not attached";
  private refreshTimer?: NodeJS.Timeout;
  private activeRequest?: AbortController;

  constructor(private readonly context: vscode.ExtensionContext) {}

  start(): void {
    const interval = vscode.workspace
      .getConfiguration("sessionweft")
      .get<number>("refreshIntervalMs", 2000);
    this.stopTimer();
    this.refreshTimer = setInterval(() => void this.refresh(), interval);
    this.context.subscriptions.push({ dispose: () => this.stopTimer() });
    void this.refresh();
  }

  async refresh(): Promise<void> {
    const sessionId = this.context.workspaceState.get<string>(SESSION_KEY);
    if (!sessionId) {
      this.status = "Use SessionWeft: Attach Session";
      this.view = undefined;
      this.events = [];
      this.changed.fire();
      return;
    }
    this.activeRequest?.abort();
    const controller = new AbortController();
    this.activeRequest = controller;
    try {
      const client = await this.client();
      const configuration = vscode.workspace.getConfiguration("sessionweft");
      const [view, batch] = await Promise.all([
        client.clientView(
          sessionId,
          {
            agentId: nonEmpty(configuration.get<string>("agentId")),
            workflowId: nonEmpty(configuration.get<string>("workflowId")),
            workspaceId: nonEmpty(configuration.get<string>("workspaceId")),
          },
          controller.signal,
        ),
        client.events(
          this.context.workspaceState.get<number>(CURSOR_KEY, 0),
          100,
          controller.signal,
        ),
      ]);
      this.view = view;
      this.events.push(...batch.events);
      this.events = this.events.slice(-200);
      await this.context.workspaceState.update(CURSOR_KEY, batch.next);
      this.status = `Connected · cursor ${batch.next}`;
    } catch (error) {
      if (!controller.signal.aborted) {
        this.status = `Offline · ${messageOf(error)}`;
      }
    } finally {
      if (this.activeRequest === controller) this.activeRequest = undefined;
      this.changed.fire();
    }
  }

  getTreeItem(element: RuntimeNode): vscode.TreeItem {
    return element;
  }

  getChildren(element?: RuntimeNode): RuntimeNode[] {
    if (element) return element.children;
    const roots: RuntimeNode[] = [
      new RuntimeNode("Runtime", vscode.TreeItemCollapsibleState.None, [], this.status),
    ];
    if (!this.view) return roots;
    roots.push(jsonNode("Session", this.view.session));
    roots.push(jsonNode("Agent", this.view.agent ?? "Not selected"));
    roots.push(jsonNode("Workflow", this.view.workflow ?? "Not selected"));
    roots.push(jsonNode("Locks", this.view.locks));
    roots.push(
      new RuntimeNode(
        "Approvals",
        vscode.TreeItemCollapsibleState.Expanded,
        this.view.pending_approvals.map(
          (approval) =>
            new RuntimeNode(
              approval.title,
              vscode.TreeItemCollapsibleState.None,
              [],
              `${approval.node_id} · v${approval.expected_version}`,
            ),
        ),
        `${this.view.pending_approvals.length}`,
      ),
    );
    roots.push(
      new RuntimeNode(
        "Events",
        vscode.TreeItemCollapsibleState.Collapsed,
        this.events
          .slice()
          .reverse()
          .map(
            (event) =>
              new RuntimeNode(
                event.envelope.event_type,
                vscode.TreeItemCollapsibleState.None,
                [],
                `#${event.cursor}`,
              ),
          ),
        `${this.events.length}`,
      ),
    );
    return roots;
  }

  async chooseApproval(approved: boolean): Promise<void> {
    const sessionId = this.context.workspaceState.get<string>(SESSION_KEY);
    if (!sessionId || !this.view) {
      void vscode.window.showWarningMessage("Attach a SessionWeft session first.");
      return;
    }
    const approval = await vscode.window.showQuickPick(
      this.view.pending_approvals.map((item) => ({
        label: item.title,
        description: `${item.node_id} · workflow ${item.workflow_id}`,
        approval: item,
      })),
      { placeHolder: approved ? "Select work to approve" : "Select work to reject" },
    );
    if (!approval) return;
    try {
      await (await this.client()).decideApproval(sessionId, approval.approval, approved);
      void vscode.window.showInformationMessage(
        approved ? "SessionWeft approval granted." : "SessionWeft approval rejected.",
      );
      await this.refresh();
    } catch (error) {
      void vscode.window.showErrorMessage(`SessionWeft approval failed: ${messageOf(error)}`);
    }
  }

  pendingApprovals(): PendingApproval[] {
    return this.view?.pending_approvals ?? [];
  }

  dispose(): void {
    this.activeRequest?.abort();
    this.stopTimer();
    this.changed.dispose();
  }

  private async client(): Promise<RuntimeClient> {
    const endpoint = vscode.workspace
      .getConfiguration("sessionweft")
      .get<string>("endpoint", "http://127.0.0.1:7447");
    const token = await this.context.secrets.get(TOKEN_SECRET);
    return new RuntimeClient(endpoint, token);
  }

  private stopTimer(): void {
    if (this.refreshTimer) clearInterval(this.refreshTimer);
    this.refreshTimer = undefined;
  }
}

export function activate(context: vscode.ExtensionContext): void {
  const provider = new RuntimeTreeProvider(context);
  context.subscriptions.push(
    provider,
    vscode.window.registerTreeDataProvider("sessionweft.runtime", provider),
    vscode.commands.registerCommand("sessionweft.refresh", () => provider.refresh()),
    vscode.commands.registerCommand("sessionweft.attach", async () => {
      const sessionId = await vscode.window.showInputBox({
        prompt: "Session UUID",
        value: context.workspaceState.get<string>(SESSION_KEY, ""),
        validateInput: (value) =>
          /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
            value,
          )
            ? undefined
            : "Enter a valid UUID",
      });
      if (!sessionId) return;
      await context.workspaceState.update(SESSION_KEY, sessionId);
      await context.workspaceState.update(CURSOR_KEY, 0);
      await provider.refresh();
    }),
    vscode.commands.registerCommand("sessionweft.configureToken", async () => {
      const token = await vscode.window.showInputBox({
        prompt: "Runtime bearer token",
        password: true,
        ignoreFocusOut: true,
      });
      if (token === undefined) return;
      if (token.length === 0) {
        await context.secrets.delete(TOKEN_SECRET);
        void vscode.window.showInformationMessage("SessionWeft Runtime token removed.");
      } else {
        await context.secrets.store(TOKEN_SECRET, token);
        void vscode.window.showInformationMessage("SessionWeft Runtime token stored securely.");
      }
      await provider.refresh();
    }),
    vscode.commands.registerCommand("sessionweft.approve", () => provider.chooseApproval(true)),
    vscode.commands.registerCommand("sessionweft.reject", () => provider.chooseApproval(false)),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("sessionweft")) provider.start();
    }),
  );
  provider.start();
}

export function deactivate(): void {
  // Disposing the client only stops polling. Runtime-owned work is intentionally left running.
}

function jsonNode(label: string, value: unknown): RuntimeNode {
  const text = JSON.stringify(value, null, 2) ?? String(value);
  return new RuntimeNode(
    label,
    vscode.TreeItemCollapsibleState.Collapsed,
    text.split("\n").slice(0, 200).map((line) => new RuntimeNode(line, vscode.TreeItemCollapsibleState.None)),
  );
}

function nonEmpty(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
