# AI AGENT RUNTIME PLATFORM

## Kế hoạch triển khai và Task Breakdown — Bản hiệu chỉnh

---

# 1. Mục tiêu

Xây dựng một **AI Agent Runtime Platform có khả năng vận hành production**, trong đó:

- `Session` là nguồn dữ liệu trung tâm và duy nhất.
- IDE và CLI chỉ là client kết nối đến Runtime.
- AI provider có thể thay thế mà không làm mất session và context.
- Nhiều agent có thể cộng tác trên cùng một workspace.
- Agent không trực tiếp sở hữu state lâu dài.
- Runtime quản lý session, task, memory, workspace, Git state, lock, decision, event và timeline.
- Ưu tiên tái sử dụng mã nguồn mở phù hợp thay vì xây dựng lại toàn bộ hệ thống.

---

# 2. Quy ước nguồn thông tin

Trong kế hoạch này:

| Nhãn        | Ý nghĩa                                                            |
| ----------- | ------------------------------------------------------------------ |
| **PDD**     | Nội dung được xác định trực tiếp trong Project Definition Document |
| **Đề xuất** | Nội dung được bổ sung để có thể triển khai thực tế                 |
| **TBD**     | Chưa có quyết định chính thức, cần Research, ADR hoặc RFC xác nhận |

Không được xem nội dung **Đề xuất** hoặc **TBD** là quyết định kiến trúc đã được phê duyệt.

---

# 3. Các điểm đã sửa so với bản trước

## 3.1. Sửa cấu trúc giai đoạn

PDD xác định đúng thứ tự:

```text
Phase -1: Capability Matrix
Phase 0: Landscape Research
Architecture Review
ADR
Phase 1: RFC và Production Specification
Phase 2: Implementation
Testing
Release
GA
```

Không nên gộp Architecture, ADR và RFC vào một giai đoạn duy nhất.

---

## 3.2. Sửa cách hiểu về công nghệ mã nguồn mở

Các dự án như:

- 9Router.
- OmniRouter.
- LiteLLM.
- Mem0.
- Graphiti.
- Zep.
- Supermemory.
- LangGraph.
- Temporal.
- gitoxide.
- libgit2.

là các đối tượng cần nghiên cứu hoặc tái sử dụng.

Không bắt buộc phải tích hợp toàn bộ các dự án này.

Mỗi dự án phải được đánh giá để đưa ra một trong các quyết định:

- Adopt.
- Wrap.
- Fork.
- Replace.
- Reject.

---

## 3.3. Sửa phạm vi Provider Layer

Không cần triển khai toàn bộ adapter provider ngay trong phiên bản đầu tiên.

Cách triển khai hợp lý:

1. Hoàn thiện Provider SDK.
2. Chọn hai provider tham chiếu.
3. Kiểm tra khả năng hot-swap.
4. Xác nhận interface chung.
5. Sau đó mới mở rộng các adapter khác.

Provider tham chiếu cụ thể là **TBD** sau Landscape Research.

---

## 3.4. Sửa timeline

PDD không cung cấp thời gian triển khai cụ thể.

Do đó:

- Không sử dụng con số 38 tuần như timeline chính thức.
- Timeline chỉ được xem là ước lượng ban đầu.
- Ước lượng chính thức phải được lập sau Phase 0.
- Khối lượng Phase 2 phụ thuộc vào kết quả Adopt/Fork/Replace.

---

## 3.5. Sửa phạm vi RFC

PDD yêu cầu các RFC sau:

1. Runtime.
2. Session.
3. Provider SDK.
4. Agent SDK.
5. Workflow DSL.
6. Lock Protocol.
7. Event Schema.
8. Memory.
9. Workspace.
10. Plugin SDK.
11. CLI Protocol.
12. IDE Protocol.
13. Security.
14. Observability.
15. Deployment.

Các RFC về Context, Git và MCP trong bản trước là hợp lý về kỹ thuật nhưng chưa được PDD yêu cầu trực tiếp.

Các nội dung này được chuyển thành:

- RFC bổ sung, nếu Architecture Review yêu cầu.
- Hoặc section trong RFC đã có.
- Trạng thái hiện tại: **Đề xuất/TBD**.

---

## 3.6. Sửa điểm chưa rõ về Event Bus

PDD đồng thời đề cập:

- Worker giao tiếp qua local Pub/Sub.
- Technology stack sử dụng NATS.

Điểm này chưa đủ rõ để triển khai.

Cần ADR xác định một trong các mô hình:

### Phương án A

NATS là Pub/Sub duy nhất, chạy local hoặc remote.

### Phương án B

Runtime có local in-process Pub/Sub và bridge sang NATS.

### Phương án C

Local mode dùng in-process Pub/Sub, production mode dùng NATS.

Quyết định cuối cùng: **TBD qua ADR**.

---

# 4. Nguyên tắc triển khai bắt buộc

## 4.1. Nguyên tắc kiến trúc

- Session-first.
- Provider-agnostic.
- Event-driven.
- Plugin-first.
- MCP-native.
- Shared Memory.
- Shared Workspace.
- Lock-based Collaboration.
- Incremental Context.
- Production concerns được xử lý từ đầu.

## 4.2. Quy tắc ownership

```text
Runtime owns state.
Agents never own persistent state.
```

Agent có thể giữ temporary execution state trong thời gian xử lý, nhưng mọi state cần thiết để:

- Resume.
- Recovery.
- Audit.
- Handover.
- Retry.

phải được lưu vào Shared Session hoặc storage do Runtime quản lý.

---

# 5. Cấu trúc quản lý tài liệu

## 5.1. Source of Truth

`PROJECT.md` là nguồn thông tin trung tâm của dự án.

File này phải thể hiện:

- Mục tiêu hiện tại.
- Trạng thái từng phase.
- Quyết định đã phê duyệt.
- RFC và ADR đang áp dụng.
- Thành phần đã chọn.
- Thành phần bị loại.
- Rủi ro đang mở.
- Roadmap hiện tại.

## 5.2. Cấu trúc thư mục

```text
docs/
├── 00-product/
├── 01-research/
├── 02-architecture/
├── 03-rfc/
├── 04-adr/
├── 05-specs/
├── 06-api/
├── 07-sdk/
├── 08-operations/
├── 09-testing/
└── 10-deployment/
```

---

# 6. Tổng quan các giai đoạn

| Giai đoạn           | Mục tiêu                          | Điều kiện hoàn thành                  |
| ------------------- | --------------------------------- | ------------------------------------- |
| Phase -1            | Xây dựng Capability Matrix        | Capability Matrix được review         |
| Phase 0             | Nghiên cứu landscape và OSS       | Có shortlist và reuse recommendation  |
| Architecture Review | Xác lập kiến trúc tổng thể        | Architecture baseline được duyệt      |
| ADR                 | Chốt các quyết định quan trọng    | ADR quan trọng được approve           |
| Phase 1             | Hoàn thành RFC và Production Spec | Specification Gate được thông qua     |
| Phase 2             | Triển khai hệ thống               | Các subsystem đạt acceptance criteria |
| Testing             | Xác minh production readiness     | Test Gate được thông qua              |
| Release             | Chuẩn bị phát hành                | Release Candidate đạt yêu cầu         |
| GA                  | Phát hành chính thức              | Production sign-off                   |

---

# 7. Phase -1 — Capability Matrix

## 7.1. Mục tiêu

Xác định đầy đủ các capability nền tảng cần có trước khi đánh giá công nghệ.

## 7.2. Task Breakdown

| ID     | Task                                         | Đầu ra                  | Phụ thuộc       |
| ------ | -------------------------------------------- | ----------------------- | --------------- |
| P-1-01 | Tạo `PROJECT.md`                             | Source of truth ban đầu | —               |
| P-1-02 | Tạo cấu trúc `docs/`                         | Documentation structure | P-1-01          |
| P-1-03 | Tạo Research template                        | Research template       | P-1-02          |
| P-1-04 | Tạo ADR template                             | ADR template            | P-1-02          |
| P-1-05 | Tạo RFC template                             | RFC template            | P-1-02          |
| P-1-06 | Xác định capability của Coding Agent         | Capability list         | P-1-03          |
| P-1-07 | Xác định capability của Provider             | Capability list         | P-1-03          |
| P-1-08 | Xác định capability của Memory               | Capability list         | P-1-03          |
| P-1-09 | Xác định capability của Workflow             | Capability list         | P-1-03          |
| P-1-10 | Xác định capability của Workspace            | Capability list         | P-1-03          |
| P-1-11 | Xác định capability của Locking              | Capability list         | P-1-03          |
| P-1-12 | Xác định capability của Pub/Sub              | Capability list         | P-1-03          |
| P-1-13 | Xác định capability của MCP                  | Capability list         | P-1-03          |
| P-1-14 | Xác định capability của Git                  | Capability list         | P-1-03          |
| P-1-15 | Xác định capability của IDE, CLI và Terminal | Capability list         | P-1-03          |
| P-1-16 | Định nghĩa tiêu chí production readiness     | Evaluation criteria     | P-1-06 → P-1-15 |
| P-1-17 | Hoàn thiện Capability Matrix                 | Capability Matrix       | P-1-16          |
| P-1-18 | Review Capability Matrix                     | Review record           | P-1-17          |

## 7.3. Cấu trúc Capability Matrix

| Trường                    | Mô tả                           |
| ------------------------- | ------------------------------- |
| Capability ID             | Mã capability                   |
| Domain                    | Nhóm chức năng                  |
| Description               | Mô tả                           |
| Priority                  | Must/Should/Could               |
| Production requirement    | Yêu cầu production              |
| Security requirement      | Yêu cầu bảo mật                 |
| Performance requirement   | Yêu cầu hiệu năng               |
| Observability requirement | Yêu cầu logging/metrics/tracing |
| OSS candidate             | Ứng viên mã nguồn mở            |
| Acceptance criteria       | Tiêu chí nghiệm thu             |
| Status                    | Open/Reviewed/Approved          |

## 7.4. Gate Phase -1

Phase -1 chỉ hoàn thành khi:

- Bao phủ đủ các research category trong PDD.
- Không còn capability quan trọng chỉ tồn tại trong trao đổi miệng.
- Mỗi capability có mức ưu tiên.
- Mỗi capability có acceptance criteria sơ bộ.
- Capability Matrix đã được review.

---

# 8. Phase 0 — Landscape Research

## 8.1. Mục tiêu

Đánh giá các dự án, framework và protocol hiện có trước khi quyết định xây mới.

## 8.2. Nhóm nghiên cứu

| ID   | Nhóm nghiên cứu          |
| ---- | ------------------------ |
| R-01 | Coding Agents            |
| R-02 | AI Providers             |
| R-03 | Provider Routing         |
| R-04 | Memory                   |
| R-05 | Workflow                 |
| R-06 | Workspace                |
| R-07 | Locking                  |
| R-08 | Pub/Sub                  |
| R-09 | MCP                      |
| R-10 | Git                      |
| R-11 | IDE                      |
| R-12 | CLI và Terminal          |
| R-13 | Code parsing và indexing |
| R-14 | Vector database          |

## 8.3. Task Breakdown

| ID    | Task                                                | Đầu ra                    |
| ----- | --------------------------------------------------- | ------------------------- |
| P0-01 | Nghiên cứu Coding Agent architecture                | Research report           |
| P0-02 | Nghiên cứu provider SDK hiện có                     | Research report           |
| P0-03 | Đánh giá 9Router, OmniRouter, LiteLLM và OpenRouter | Provider routing matrix   |
| P0-04 | Đánh giá Mem0, Graphiti, Zep và Supermemory         | Memory matrix             |
| P0-05 | Đánh giá codebase-memory                            | Repository memory report  |
| P0-06 | Đánh giá LangGraph và Temporal                      | Workflow comparison       |
| P0-07 | Đánh giá NATS và phương án local Pub/Sub            | Event architecture report |
| P0-08 | Đánh giá MCP ecosystem                              | MCP report                |
| P0-09 | Đánh giá tree-sitter                                | Parsing report            |
| P0-10 | Đánh giá LSP integration                            | LSP report                |
| P0-11 | Đánh giá Tantivy và ripgrep                         | Search report             |
| P0-12 | Đánh giá Qdrant                                     | Vector database report    |
| P0-13 | Đánh giá gitoxide và libgit2                        | Git integration report    |
| P0-14 | Đánh giá locking và lease model                     | Locking report            |
| P0-15 | Đánh giá VS Code Extension architecture             | IDE report                |
| P0-16 | Đánh giá clap và Ratatui                            | CLI/TUI report            |
| P0-17 | Tổng hợp license compatibility                      | License matrix            |
| P0-18 | Tổng hợp security và maintenance risk               | Risk matrix               |
| P0-19 | Chấm reuse score                                    | Reuse scorecard           |
| P0-20 | Đề xuất Adopt/Wrap/Fork/Replace/Reject              | Recommendation report     |
| P0-21 | Ước lượng implementation effort                     | Effort estimate           |
| P0-22 | Review kết quả Phase 0                              | Research approval record  |

## 8.4. Tiêu chí đánh giá bắt buộc

Mỗi đối tượng nghiên cứu phải được đánh giá theo:

- Architecture.
- License.
- Community.
- Maintenance.
- Release frequency.
- Security history.
- Documentation.
- Extensibility.
- Performance.
- Production readiness.
- Integration effort.
- Vendor lock-in.
- Pros.
- Cons.
- Reuse score.
- Recommendation.

## 8.5. Gate Phase 0

Phase 0 hoàn thành khi:

- Có kết quả nghiên cứu cho mọi category.
- Có license matrix.
- Có shortlist.
- Có lý do rõ ràng cho từng quyết định reuse hoặc tự xây.
- Có effort estimate cập nhật.
- Có danh sách rủi ro kiến trúc.
- Có đủ dữ liệu để thực hiện Architecture Review.

---

# 9. Architecture Review

## 9.1. Mục tiêu

Tạo kiến trúc tổng thể dựa trên kết quả nghiên cứu, không dựa trên giả định ban đầu.

## 9.2. Task Breakdown

| ID    | Task                                | Đầu ra                     |
| ----- | ----------------------------------- | -------------------------- |
| AR-01 | Thiết kế System Context Diagram     | Context diagram            |
| AR-02 | Thiết kế Container Diagram          | Container diagram          |
| AR-03 | Xác định bounded domains            | Domain map                 |
| AR-04 | Thiết kế Shared Session model       | Session architecture       |
| AR-05 | Thiết kế Agent Runtime model        | Agent architecture         |
| AR-06 | Thiết kế Workflow architecture      | Workflow architecture      |
| AR-07 | Thiết kế Event architecture         | Event architecture         |
| AR-08 | Thiết kế Workspace architecture     | Workspace architecture     |
| AR-09 | Thiết kế Collaboration architecture | Collaboration architecture |
| AR-10 | Thiết kế Memory architecture        | Memory architecture        |
| AR-11 | Thiết kế Context Engine             | Context architecture       |
| AR-12 | Thiết kế Provider Layer             | Provider architecture      |
| AR-13 | Thiết kế Plugin và MCP architecture | Plugin architecture        |
| AR-14 | Thiết kế CLI và IDE communication   | Client architecture        |
| AR-15 | Thiết kế deployment topology        | Deployment architecture    |
| AR-16 | Thực hiện threat modeling ban đầu   | Threat model               |
| AR-17 | Xác định architecture risks         | Risk register              |
| AR-18 | Architecture Review                 | Review record              |
| AR-19 | Chỉnh sửa sau review                | Approved baseline          |

## 9.3. Gate Architecture

Không được chuyển sang ADR khi:

- Session ownership chưa rõ.
- Runtime và client responsibility chưa rõ.
- Agent state model chưa rõ.
- Local Pub/Sub và NATS chưa được phân biệt.
- Database deployment mode chưa rõ.
- Provider switching chưa có kiến trúc.
- Lock scope chưa rõ.
- Recovery model chưa rõ.

---

# 10. ADR Phase

## 10.1. ADR bắt buộc

| ADR     | Nội dung                                 | Trạng thái ban đầu             |
| ------- | ---------------------------------------- | ------------------------------ |
| ADR-001 | Runtime sử dụng Rust và Tokio            | PDD, cần xác nhận sau research |
| ADR-002 | RPC sử dụng gRPC và Protobuf             | PDD, cần xác nhận              |
| ADR-003 | Event architecture và vai trò của NATS   | TBD                            |
| ADR-004 | SQLite WAL và PostgreSQL deployment mode | TBD                            |
| ADR-005 | Session là single source of truth        | PDD                            |
| ADR-006 | Agent persistent state ownership         | PDD                            |
| ADR-007 | Provider adapter architecture            | TBD                            |
| ADR-008 | Provider routing reuse strategy          | TBD                            |
| ADR-009 | Hierarchical lock model                  | PDD                            |
| ADR-010 | Workflow custom DAG strategy             | PDD                            |
| ADR-011 | Memory provider strategy                 | TBD                            |
| ADR-012 | Qdrant usage                             | PDD, cần xác nhận              |
| ADR-013 | tree-sitter, LSP và Tantivy integration  | PDD, cần xác nhận              |
| ADR-014 | Git library selection                    | TBD                            |
| ADR-015 | MCP architecture                         | PDD                            |
| ADR-016 | Plugin isolation strategy                | TBD                            |
| ADR-017 | Local và distributed deployment mode     | TBD                            |
| ADR-018 | Failure recovery strategy                | TBD                            |

## 10.2. Cấu trúc ADR

Mỗi ADR phải có:

- Context.
- Problem.
- Decision.
- Alternatives.
- Decision drivers.
- Consequences.
- Risks.
- Migration impact.
- Rollback hoặc replacement strategy.
- Status.
- Approvers.

## 10.3. Gate ADR

Không được viết RFC implementation-level khi ADR liên quan chưa được approve.

---

# 11. Phase 1 — RFC và Production Specification

## 11.1. RFC bắt buộc theo PDD

| ID      | RFC              |
| ------- | ---------------- |
| RFC-001 | Runtime Core     |
| RFC-002 | Session Engine   |
| RFC-003 | Provider SDK     |
| RFC-004 | Agent SDK        |
| RFC-005 | Workflow DSL     |
| RFC-006 | Lock Protocol    |
| RFC-007 | Event Schema     |
| RFC-008 | Memory Engine    |
| RFC-009 | Workspace Engine |
| RFC-010 | Plugin SDK       |
| RFC-011 | CLI Protocol     |
| RFC-012 | IDE Protocol     |
| RFC-013 | Security         |
| RFC-014 | Observability    |
| RFC-015 | Deployment       |

## 11.2. RFC bổ sung đề xuất

| ID      | RFC             | Điều kiện tạo                                        |
| ------- | --------------- | ---------------------------------------------------- |
| RFC-X01 | Context Engine  | Khi Context không thể mô tả đầy đủ trong Session RFC |
| RFC-X02 | Git Integration | Khi Git state và merge flow đủ phức tạp              |
| RFC-X03 | MCP Runtime     | Khi MCP không thể mô tả đầy đủ trong Plugin RFC      |
| RFC-X04 | Storage Model   | Khi SQLite/PostgreSQL cần contract độc lập           |

## 11.3. Task Breakdown

| ID    | Task                              |
| ----- | --------------------------------- |
| P1-01 | Viết Runtime Core RFC             |
| P1-02 | Viết Session Engine RFC           |
| P1-03 | Viết Provider SDK RFC             |
| P1-04 | Viết Agent SDK RFC                |
| P1-05 | Viết Workflow DSL RFC             |
| P1-06 | Viết Lock Protocol RFC            |
| P1-07 | Viết Event Schema RFC             |
| P1-08 | Viết Memory Engine RFC            |
| P1-09 | Viết Workspace Engine RFC         |
| P1-10 | Viết Plugin SDK RFC               |
| P1-11 | Viết CLI Protocol RFC             |
| P1-12 | Viết IDE Protocol RFC             |
| P1-13 | Viết Security RFC                 |
| P1-14 | Viết Observability RFC            |
| P1-15 | Viết Deployment RFC               |
| P1-16 | Xác định RFC bổ sung cần thiết    |
| P1-17 | Review xung đột giữa các RFC      |
| P1-18 | Xây dựng Production Specification |
| P1-19 | Xây dựng API Specification        |
| P1-20 | Xây dựng Event Catalog            |
| P1-21 | Xây dựng Error Catalog            |
| P1-22 | Xây dựng Security Control Matrix  |
| P1-23 | Xây dựng Observability Matrix     |
| P1-24 | Xây dựng Test Strategy            |
| P1-25 | Specification Review              |
| P1-26 | Specification Approval            |

## 11.4. Production Specification phải bao gồm

- Availability target.
- Recovery target.
- Data durability.
- Scalability.
- Security controls.
- Audit requirements.
- Logging.
- Metrics.
- Tracing.
- Rate limiting.
- Cost tracking.
- Upgrade strategy.
- Migration strategy.
- Backup và restore.
- Failure handling.
- Compatibility policy.
- Performance target.

Các giá trị cụ thể như SLA, RTO và RPO hiện là **TBD**.

## 11.5. Specification Gate

Phase 2 chỉ được bắt đầu khi:

- RFC quan trọng đã approve.
- Event schema đã version hóa.
- Session state machine đã rõ.
- Provider SDK contract đã rõ.
- Lock protocol đã rõ.
- Workflow persistence đã rõ.
- Failure recovery đã rõ.
- Security control đã rõ.
- Test strategy đã được duyệt.

---

# 12. Phase 2 — Implementation

Phase 2 được chia thành các workstream. Các workstream có thể chạy song song nhưng phải tuân thủ dependency.

---

# 13. Workstream A — Runtime Foundation

## Tasks

| ID    | Task                                    | Phụ thuộc          |
| ----- | --------------------------------------- | ------------------ |
| RT-01 | Khởi tạo Rust workspace                 | Specification Gate |
| RT-02 | Thiết lập Tokio runtime                 | RT-01              |
| RT-03 | Implement configuration system          | RT-01              |
| RT-04 | Implement service lifecycle             | RT-02              |
| RT-05 | Implement service registry              | RT-04              |
| RT-06 | Implement shared error model            | RT-01              |
| RT-07 | Implement cancellation model            | RT-02              |
| RT-08 | Implement timeout model                 | RT-02              |
| RT-09 | Implement graceful shutdown             | RT-04              |
| RT-10 | Implement health và readiness           | RT-05              |
| RT-11 | Implement structured logging foundation | RT-01              |
| RT-12 | Implement tracing foundation            | RT-01              |
| RT-13 | Viết Runtime unit tests                 | RT-01 → RT-12      |
| RT-14 | Viết Runtime integration tests          | RT-05 → RT-12      |

## Acceptance Criteria

- Runtime khởi động và dừng an toàn.
- Có cancellation và timeout.
- Có health và readiness.
- Có structured logging.
- Có correlation hoặc trace ID.
- Không để state quan trọng trong process memory mà không có recovery strategy.

---

# 14. Workstream B — RPC và API Contract

## Tasks

| ID     | Task                                 | Phụ thuộc       |
| ------ | ------------------------------------ | --------------- |
| RPC-01 | Tạo Protobuf package structure       | RFC-001         |
| RPC-02 | Định nghĩa Runtime Service           | RPC-01          |
| RPC-03 | Định nghĩa Session Service           | RPC-01          |
| RPC-04 | Định nghĩa Agent Service             | RPC-01          |
| RPC-05 | Định nghĩa Workflow Service          | RPC-01          |
| RPC-06 | Định nghĩa Workspace Service         | RPC-01          |
| RPC-07 | Định nghĩa Memory Service            | RPC-01          |
| RPC-08 | Định nghĩa streaming protocol        | RPC-01          |
| RPC-09 | Implement gRPC server                | RT-05           |
| RPC-10 | Implement authentication interceptor | RPC-09          |
| RPC-11 | Implement authorization interceptor  | RPC-09          |
| RPC-12 | Implement tracing interceptor        | RPC-09          |
| RPC-13 | Implement compatibility tests        | RPC-02 → RPC-12 |

## Acceptance Criteria

- CLI và IDE không truy cập database trực tiếp.
- API có versioning.
- Streaming hỗ trợ cancel.
- Streaming có reconnect hoặc resume strategy.
- Error trả về có mã lỗi chuẩn.

---

# 15. Workstream C — Event Infrastructure

## Tasks

| ID     | Task                                    | Phụ thuộc          |
| ------ | --------------------------------------- | ------------------ |
| EVT-01 | Implement event envelope                | RFC-007            |
| EVT-02 | Implement event metadata                | EVT-01             |
| EVT-03 | Implement local event interface         | ADR-003            |
| EVT-04 | Implement NATS adapter nếu được chọn    | ADR-003            |
| EVT-05 | Implement publisher                     | EVT-03 hoặc EVT-04 |
| EVT-06 | Implement subscriber                    | EVT-03 hoặc EVT-04 |
| EVT-07 | Implement retry policy                  | EVT-05, EVT-06     |
| EVT-08 | Implement dead-letter handling          | EVT-07             |
| EVT-09 | Implement event deduplication           | EVT-05             |
| EVT-10 | Implement idempotency support           | EVT-06             |
| EVT-11 | Implement ordering strategy             | EVT-05             |
| EVT-12 | Implement event replay nếu được yêu cầu | RFC-007            |
| EVT-13 | Viết event contract tests               | EVT-01 → EVT-12    |

## Event tối thiểu

- `TaskCreated`
- `TaskAssigned`
- `TaskStarted`
- `TaskCompleted`
- `TaskFailed`
- `LockAcquired`
- `LockReleased`
- `ContextUpdated`
- `FileChanged`
- `ReviewRequested`
- `ReviewApproved`
- `MemoryUpdated`
- `ProviderSwitched`

---

# 16. Workstream D — Storage và Session Engine

## Tasks

| ID     | Task                                     | Phụ thuộc          |
| ------ | ---------------------------------------- | ------------------ |
| SES-01 | Thiết kế Session aggregate               | RFC-002            |
| SES-02 | Thiết kế Session state machine           | SES-01             |
| SES-03 | Thiết kế persistence model               | RFC-002            |
| SES-04 | Implement SQLite WAL storage mode        | ADR-004            |
| SES-05 | Implement PostgreSQL storage mode        | ADR-004            |
| SES-06 | Implement migration framework            | SES-04 hoặc SES-05 |
| SES-07 | Implement create session                 | SES-03             |
| SES-08 | Implement load session                   | SES-07             |
| SES-09 | Implement update session                 | SES-07             |
| SES-10 | Implement version/concurrency control    | SES-09             |
| SES-11 | Implement session timeline               | SES-09, EVT-05     |
| SES-12 | Implement session snapshot               | SES-09             |
| SES-13 | Implement session resume                 | SES-12             |
| SES-14 | Implement session archive                | SES-09             |
| SES-15 | Implement export/import nếu được yêu cầu | RFC-002            |
| SES-16 | Implement crash recovery                 | SES-12             |
| SES-17 | Viết session consistency tests           | SES-07 → SES-16    |
| SES-18 | Viết crash recovery tests                | SES-13, SES-16     |

## Session phải quản lý

- Conversation.
- History.
- Summary.
- Memory references.
- Tasks.
- Agents.
- Workflow state.
- Workspace graph.
- Git state.
- Locks.
- Decisions.
- Provider history.
- Usage.
- Cost.
- Timeline.

## Acceptance Criteria

- Runtime restart không làm mất session đã persist.
- Đổi provider không đổi session ID.
- Agent crash không làm mất task state.
- Session có concurrency protection.
- Session có recovery path rõ ràng.

---

# 17. Workstream E — Provider Layer

## Common Interface

```text
chat
stream
tool
interrupt
cancel
resume
usage
cost
rate_limit
```

## Tasks

| ID     | Task                                 | Phụ thuộc        |
| ------ | ------------------------------------ | ---------------- |
| PRV-01 | Định nghĩa Provider trait            | RFC-003          |
| PRV-02 | Định nghĩa Provider Capability Model | PRV-01           |
| PRV-03 | Định nghĩa normalized message        | PRV-01           |
| PRV-04 | Định nghĩa normalized tool call      | PRV-01           |
| PRV-05 | Định nghĩa usage và cost model       | PRV-01           |
| PRV-06 | Implement Provider Registry          | PRV-01           |
| PRV-07 | Implement Reference Provider A       | Phase 0 decision |
| PRV-08 | Implement Reference Provider B       | Phase 0 decision |
| PRV-09 | Implement provider health check      | PRV-06           |
| PRV-10 | Implement rate-limit tracking        | PRV-06           |
| PRV-11 | Implement usage tracking             | PRV-05           |
| PRV-12 | Implement cost tracking              | PRV-05           |
| PRV-13 | Implement fallback policy            | RFC-003          |
| PRV-14 | Implement provider hot-swap          | SES-09           |
| PRV-15 | Publish `ProviderSwitched` event     | PRV-14           |
| PRV-16 | Viết provider conformance tests      | PRV-07, PRV-08   |
| PRV-17 | Viết hot-swap tests                  | PRV-14           |
| PRV-18 | Mở rộng adapter theo roadmap         | PRV-16           |

## Acceptance Criteria

- Hai reference provider dùng chung Provider SDK.
- Có thể chuyển provider trong cùng session.
- Context không bị mất khi chuyển.
- Usage và cost được chuẩn hóa.
- Runtime Core không phải sửa khi thêm adapter đúng contract.

---

# 18. Workstream F — Agent Runtime

## Agent flow mục tiêu

```text
Planner
→ Architect
→ Fan-out Workers
→ Reviewer
→ Tester
→ Merge
```

## Tasks

| ID     | Task                          | Phụ thuộc       |
| ------ | ----------------------------- | --------------- |
| AGT-01 | Định nghĩa Agent Manifest     | RFC-004         |
| AGT-02 | Định nghĩa Agent Capability   | AGT-01          |
| AGT-03 | Implement Agent Registry      | AGT-01          |
| AGT-04 | Implement agent lifecycle     | RT-05           |
| AGT-05 | Implement agent heartbeat     | AGT-04          |
| AGT-06 | Implement task assignment     | SES-09, EVT-05  |
| AGT-07 | Implement agent cancel        | RT-07           |
| AGT-08 | Implement agent resume        | SES-13          |
| AGT-09 | Implement failure recovery    | AGT-04          |
| AGT-10 | Implement Planner contract    | RFC-004         |
| AGT-11 | Implement Architect contract  | RFC-004         |
| AGT-12 | Implement Worker contract     | RFC-004         |
| AGT-13 | Implement Reviewer contract   | RFC-004         |
| AGT-14 | Implement Tester contract     | RFC-004         |
| AGT-15 | Implement Merge role contract | RFC-004         |
| AGT-16 | Implement audit timeline      | SES-11          |
| AGT-17 | Viết agent lifecycle tests    | AGT-04 → AGT-09 |
| AGT-18 | Viết multi-agent tests        | AGT-10 → AGT-16 |

## Acceptance Criteria

- Agent không sở hữu persistent state riêng.
- Agent chỉ phối hợp qua Shared Session và event mechanism đã được approve.
- Agent failure có recovery.
- Có thể audit toàn bộ task transition.
- Agent capability được kiểm soát.

---

# 19. Workstream G — Workflow Engine

## Chức năng bắt buộc

- YAML workflow.
- DAG.
- Retry.
- Fallback.
- Conditional branch.
- Approval.
- Rollback.
- Parallel fan-out.
- Fan-in.
- Resume.

## Tasks

| ID    | Task                            | Phụ thuộc     |
| ----- | ------------------------------- | ------------- |
| WF-01 | Định nghĩa Workflow YAML Schema | RFC-005       |
| WF-02 | Implement YAML parser           | WF-01         |
| WF-03 | Implement schema validation     | WF-02         |
| WF-04 | Implement DAG builder           | WF-03         |
| WF-05 | Implement cycle detection       | WF-04         |
| WF-06 | Implement scheduler             | WF-04         |
| WF-07 | Implement task dependency       | WF-06         |
| WF-08 | Implement parallel fan-out      | WF-06         |
| WF-09 | Implement fan-in                | WF-08         |
| WF-10 | Implement retry và backoff      | WF-06         |
| WF-11 | Implement fallback              | WF-06         |
| WF-12 | Implement conditional branch    | WF-06         |
| WF-13 | Implement approval step         | WF-06         |
| WF-14 | Implement rollback              | WF-06         |
| WF-15 | Implement workflow persistence  | SES-09        |
| WF-16 | Implement workflow resume       | SES-13, WF-15 |
| WF-17 | Viết workflow recovery tests    | WF-15, WF-16  |
| WF-18 | Viết idempotency tests          | WF-10         |

## Acceptance Criteria

- Workflow không chạy lại task hoàn tất sau restart.
- Retry không tạo side effect trùng.
- DAG không chấp nhận cycle.
- Approval được lưu vào session.
- Workflow có thể resume.

---

# 20. Workstream H — Workspace Engine

## Tasks

| ID    | Task                              | Phụ thuộc        |
| ----- | --------------------------------- | ---------------- |
| WS-01 | Định nghĩa Workspace model        | RFC-009          |
| WS-02 | Implement workspace discovery     | WS-01            |
| WS-03 | Implement directory graph         | WS-02            |
| WS-04 | Implement file graph              | WS-02            |
| WS-05 | Integrate tree-sitter             | ADR-013          |
| WS-06 | Integrate LSP                     | ADR-013          |
| WS-07 | Implement symbol graph            | WS-05, WS-06     |
| WS-08 | Integrate Tantivy nếu được chọn   | ADR-013          |
| WS-09 | Integrate ripgrep                 | Phase 0 decision |
| WS-10 | Implement file watcher            | WS-02            |
| WS-11 | Publish `FileChanged` event       | WS-10            |
| WS-12 | Implement incremental indexing    | WS-07, WS-10     |
| WS-13 | Persist workspace graph reference | SES-09           |
| WS-14 | Viết repository scale benchmark   | WS-12            |

## Acceptance Criteria

- Runtime xác định được file và symbol liên quan.
- Chỉ index lại phần thay đổi.
- Workspace state liên kết với session.
- File change phát event.
- Có benchmark trên repository thực tế.

---

# 21. Workstream I — Collaboration và Locking

## Lock hierarchy

```text
Workspace
└── Directory
    └── File
        └── Symbol
```

## Tasks

| ID      | Task                                  | Phụ thuộc         |
| ------- | ------------------------------------- | ----------------- |
| LOCK-01 | Định nghĩa lock resource              | RFC-006           |
| LOCK-02 | Định nghĩa compatibility matrix       | LOCK-01           |
| LOCK-03 | Implement acquire lock                | LOCK-01           |
| LOCK-04 | Implement release lock                | LOCK-03           |
| LOCK-05 | Implement lease                       | LOCK-03           |
| LOCK-06 | Implement heartbeat                   | LOCK-05           |
| LOCK-07 | Implement timeout                     | LOCK-05           |
| LOCK-08 | Implement stale lock cleanup          | LOCK-06, LOCK-07  |
| LOCK-09 | Implement ownership transfer          | LOCK-03           |
| LOCK-10 | Implement hierarchical conflict check | LOCK-02           |
| LOCK-11 | Implement lock wait queue             | LOCK-10           |
| LOCK-12 | Publish lock events                   | LOCK-03, LOCK-04  |
| LOCK-13 | Implement merge queue                 | LOCK-03           |
| LOCK-14 | Định nghĩa conflict resolver contract | LOCK-13           |
| LOCK-15 | Persist lock state                    | SES-09            |
| LOCK-16 | Viết concurrency tests                | LOCK-03 → LOCK-15 |
| LOCK-17 | Viết agent crash lock-release tests   | LOCK-08           |

## Acceptance Criteria

- Lock xung đột không được cấp đồng thời.
- Lock có lease, heartbeat và timeout.
- Agent crash không giữ lock vô hạn.
- Lock state có audit.
- Merge queue xử lý theo policy rõ ràng.

---

# 22. Workstream J — Git Integration

Git Integration là subsystem cần thiết vì PDD yêu cầu Session lưu Git state, nhưng thư viện và chi tiết triển khai còn **TBD**.

## Tasks

| ID     | Task                                       | Phụ thuộc       |
| ------ | ------------------------------------------ | --------------- |
| GIT-01 | Chọn gitoxide, libgit2 hoặc phương án khác | ADR-014         |
| GIT-02 | Implement repository status                | GIT-01          |
| GIT-03 | Implement diff                             | GIT-01          |
| GIT-04 | Implement branch hoặc worktree strategy    | ADR-014         |
| GIT-05 | Implement commit operation                 | GIT-01          |
| GIT-06 | Implement merge operation                  | GIT-01          |
| GIT-07 | Integrate merge queue                      | LOCK-13         |
| GIT-08 | Implement conflict detection               | GIT-06          |
| GIT-09 | Implement rollback                         | GIT-05, GIT-06  |
| GIT-10 | Persist Git state reference                | SES-09          |
| GIT-11 | Viết Git integration tests                 | GIT-02 → GIT-10 |

## Acceptance Criteria

- Git operation liên kết với session, task và agent.
- Không merge nếu vi phạm lock policy.
- Có conflict detection.
- Có rollback hoặc recovery strategy.
- Git state có thể audit.

---

# 23. Workstream K — Memory Engine

## Memory class

- Conversation Memory.
- Repository Memory.
- Decision Memory.
- Preference Memory.
- Error Memory.

## Tasks

| ID     | Task                                   | Phụ thuộc              |
| ------ | -------------------------------------- | ---------------------- |
| MEM-01 | Định nghĩa Memory interface            | RFC-008                |
| MEM-02 | Định nghĩa Memory metadata             | MEM-01                 |
| MEM-03 | Định nghĩa lifecycle và retention      | MEM-01                 |
| MEM-04 | Chọn memory solution                   | Phase 0 recommendation |
| MEM-05 | Implement memory adapter               | MEM-04                 |
| MEM-06 | Implement Qdrant adapter nếu được chọn | ADR-012                |
| MEM-07 | Implement memory indexing              | MEM-05                 |
| MEM-08 | Implement memory retrieval             | MEM-07                 |
| MEM-09 | Implement memory ranking               | MEM-08                 |
| MEM-10 | Implement memory deduplication         | MEM-07                 |
| MEM-11 | Implement expiration và deletion       | MEM-03                 |
| MEM-12 | Implement sensitive-data filter        | RFC-013                |
| MEM-13 | Publish `MemoryUpdated` event          | MEM-07                 |
| MEM-14 | Viết retrieval quality benchmark       | MEM-08, MEM-09         |
| MEM-15 | Viết privacy và deletion tests         | MEM-11, MEM-12         |

## Acceptance Criteria

- Memory có source và timestamp.
- Memory có retention hoặc deletion policy.
- Secret không được lưu như memory thông thường.
- Memory adapter có thể thay thế.
- Retrieval có quality benchmark.

---

# 24. Workstream L — Context Engine

Context Engine là chức năng cốt lõi được mô tả trong PDD, dù chưa được liệt kê thành RFC độc lập.

## Nguyên tắc

Không broadcast toàn bộ history.

Context được xây dựng từ:

- Task hiện tại.
- Summary.
- Dependencies.
- Relevant files.
- Relevant symbols.
- Memories.
- Decisions.
- Active locks.
- Workflow state.
- Git diff.

## Tasks

| ID     | Task                                | Phụ thuộc            |
| ------ | ----------------------------------- | -------------------- |
| CTX-01 | Định nghĩa Context Request          | Session hoặc RFC-X01 |
| CTX-02 | Định nghĩa Context Item             | CTX-01               |
| CTX-03 | Implement task context loader       | SES-09               |
| CTX-04 | Implement summary loader            | SES-09               |
| CTX-05 | Implement dependency loader         | WF-07                |
| CTX-06 | Implement file và symbol loader     | WS-07                |
| CTX-07 | Implement memory loader             | MEM-08               |
| CTX-08 | Implement lock loader               | LOCK-15              |
| CTX-09 | Implement Git diff loader           | GIT-03               |
| CTX-10 | Implement token budget              | CTX-02               |
| CTX-11 | Implement context ranking           | CTX-03 → CTX-10      |
| CTX-12 | Implement context compression       | CTX-11               |
| CTX-13 | Implement provider-aware formatting | PRV-02               |
| CTX-14 | Publish `ContextUpdated` event      | CTX-11               |
| CTX-15 | Viết relevance benchmark            | CTX-11               |
| CTX-16 | Viết token-cost benchmark           | CTX-10               |

## Acceptance Criteria

- Không gửi full history mặc định.
- Context tuân thủ token budget.
- Không loại bỏ requirement quan trọng.
- Context tương thích capability của provider.
- Có benchmark về relevance và token cost.

---

# 25. Workstream M — MCP và Plugin SDK

## Tasks

| ID     | Task                                | Phụ thuộc               |
| ------ | ----------------------------------- | ----------------------- |
| PLG-01 | Định nghĩa Plugin Manifest          | RFC-010                 |
| PLG-02 | Định nghĩa Plugin Lifecycle         | PLG-01                  |
| PLG-03 | Định nghĩa permission model         | RFC-013                 |
| PLG-04 | Implement Plugin Registry           | PLG-01                  |
| PLG-05 | Implement load và unload            | PLG-04                  |
| PLG-06 | Implement version compatibility     | PLG-04                  |
| PLG-07 | Implement MCP client                | Plugin RFC hoặc RFC-X03 |
| PLG-08 | Implement MCP server bridge nếu cần | ADR-015                 |
| PLG-09 | Implement tool discovery            | PLG-07                  |
| PLG-10 | Implement permission enforcement    | PLG-03                  |
| PLG-11 | Implement timeout và cancellation   | RT-07, RT-08            |
| PLG-12 | Implement plugin isolation          | ADR-016                 |
| PLG-13 | Implement tool audit log            | RFC-013                 |
| PLG-14 | Viết malicious plugin tests         | PLG-10, PLG-12          |

## Acceptance Criteria

- Plugin không có toàn quyền mặc định.
- Tool execution có permission check.
- Tool có timeout và cancellation.
- Plugin lỗi không làm Runtime Core dừng.
- Tool action có audit trail.

---

# 26. Workstream N — CLI và TUI

## Tasks

| ID     | Task                               | Phụ thuộc       |
| ------ | ---------------------------------- | --------------- |
| CLI-01 | Thiết kế command tree              | RFC-011         |
| CLI-02 | Khởi tạo clap CLI                  | CLI-01          |
| CLI-03 | Implement Runtime connection       | RPC-09          |
| CLI-04 | Implement session create/list/open | SES-07 → SES-09 |
| CLI-05 | Implement session resume           | SES-13          |
| CLI-06 | Implement provider switch          | PRV-14          |
| CLI-07 | Implement agent run và status      | AGT-04          |
| CLI-08 | Implement workflow run và status   | WF-06           |
| CLI-09 | Implement lock status              | LOCK-15         |
| CLI-10 | Implement event stream             | EVT-06          |
| CLI-11 | Khởi tạo Ratatui TUI               | CLI-03          |
| CLI-12 | Implement session panel            | CLI-11          |
| CLI-13 | Implement agent panel              | CLI-11          |
| CLI-14 | Implement workflow panel           | CLI-11          |
| CLI-15 | Implement event panel              | CLI-11          |
| CLI-16 | Implement reconnect                | RPC-08          |
| CLI-17 | Viết CLI end-to-end tests          | CLI-03 → CLI-16 |

## Acceptance Criteria

- CLI là client của Runtime.
- CLI không gọi provider trực tiếp.
- CLI không truy cập database trực tiếp.
- Có thể create, open và resume session.
- Có thể cancel operation đang chạy.
- Lỗi hiển thị correlation ID.

---

# 27. Workstream O — VS Code Extension

## Tasks

| ID     | Task                            | Phụ thuộc       |
| ------ | ------------------------------- | --------------- |
| IDE-01 | Thiết kế Extension architecture | RFC-012         |
| IDE-02 | Tạo VS Code Extension project   | IDE-01          |
| IDE-03 | Implement Runtime connection    | RPC-09          |
| IDE-04 | Implement Session Explorer      | SES-09          |
| IDE-05 | Implement Agent Explorer        | AGT-03          |
| IDE-06 | Implement Workflow Explorer     | WF-15           |
| IDE-07 | Implement conversation panel    | PRV-06          |
| IDE-08 | Implement task panel            | SES-09          |
| IDE-09 | Implement lock visualization    | LOCK-15         |
| IDE-10 | Implement workspace change view | WS-11           |
| IDE-11 | Implement provider switch       | PRV-14          |
| IDE-12 | Implement approval interaction  | WF-13           |
| IDE-13 | Implement reconnect và resume   | SES-13          |
| IDE-14 | Viết Extension end-to-end tests | IDE-03 → IDE-13 |

## Acceptance Criteria

- Đóng IDE không làm session bị mất.
- Mở lại IDE có thể reconnect.
- IDE chỉ là client.
- Hiển thị agent, task, workflow và lock.
- Có thể chuyển provider trong session.

---

# 28. Workstream P — Security

## Tasks

| ID     | Task                                       |
| ------ | ------------------------------------------ |
| SEC-01 | Hoàn thiện threat model                    |
| SEC-02 | Implement client authentication            |
| SEC-03 | Implement authorization                    |
| SEC-04 | Implement secret management                |
| SEC-05 | Implement provider credential isolation    |
| SEC-06 | Implement plugin permission enforcement    |
| SEC-07 | Implement tool execution policy            |
| SEC-08 | Implement audit logging                    |
| SEC-09 | Implement sensitive-data redaction         |
| SEC-10 | Implement session access control           |
| SEC-11 | Implement dependency scanning              |
| SEC-12 | Implement artifact hoặc container scanning |
| SEC-13 | Thực hiện security testing                 |
| SEC-14 | Tạo incident response procedure            |

## Acceptance Criteria

- Secret không xuất hiện trong log, event hoặc memory.
- Session có access control.
- Tool nguy hiểm cần permission.
- Hành động quan trọng có audit.
- Không release khi còn vulnerability nghiêm trọng chưa xử lý.

---

# 29. Workstream Q — Observability

## Tasks

| ID     | Task                         |
| ------ | ---------------------------- |
| OBS-01 | Định nghĩa log schema        |
| OBS-02 | Implement structured logging |
| OBS-03 | Implement correlation ID     |
| OBS-04 | Implement tracing            |
| OBS-05 | Implement metrics            |
| OBS-06 | Theo dõi session latency     |
| OBS-07 | Theo dõi provider latency    |
| OBS-08 | Theo dõi provider errors     |
| OBS-09 | Theo dõi usage và cost       |
| OBS-10 | Theo dõi event lag           |
| OBS-11 | Theo dõi lock contention     |
| OBS-12 | Theo dõi workflow failures   |
| OBS-13 | Tạo dashboard                |
| OBS-14 | Tạo alert                    |
| OBS-15 | Tạo operation runbook        |

## Metric tối thiểu

- Runtime availability.
- Request latency.
- Session load time.
- Provider response time.
- Provider error rate.
- Event processing delay.
- Token usage.
- Cost per session.
- Active agents.
- Failed agents.
- Lock wait time.
- Workflow success rate.
- Memory retrieval latency.
- Context token size.

Target cụ thể: **TBD trong Production Specification**.

---

# 30. Workstream R — Deployment và Operations

## Tasks

| ID     | Task                                       |
| ------ | ------------------------------------------ |
| DEP-01 | Tạo local development environment          |
| DEP-02 | Tạo Runtime build artifact                 |
| DEP-03 | Tạo container image nếu deployment yêu cầu |
| DEP-04 | Cấu hình NATS nếu được chọn                |
| DEP-05 | Cấu hình SQLite local mode                 |
| DEP-06 | Cấu hình PostgreSQL production mode        |
| DEP-07 | Cấu hình Qdrant nếu được chọn              |
| DEP-08 | Cấu hình secrets                           |
| DEP-09 | Cấu hình persistent storage                |
| DEP-10 | Cấu hình backup                            |
| DEP-11 | Cấu hình restore                           |
| DEP-12 | Cấu hình rolling update                    |
| DEP-13 | Cấu hình rollback                          |
| DEP-14 | Tạo disaster recovery procedure            |
| DEP-15 | Tạo staging environment                    |
| DEP-16 | Tạo production deployment definition       |

## Acceptance Criteria

- Local environment có thể khởi động bằng quy trình rõ ràng.
- Staging gần tương đương production.
- Upgrade không làm mất session.
- Backup và restore được kiểm thử.
- Có rollback procedure.
- Production secret không nằm trong source code.

---

# 31. Testing Phase

## 31.1. Nhóm kiểm thử

| Nhóm               | Phạm vi                             |
| ------------------ | ----------------------------------- |
| Unit Test          | Domain logic                        |
| Contract Test      | RPC, event, provider và plugin      |
| Integration Test   | Runtime với infrastructure          |
| End-to-End Test    | CLI/IDE đến Runtime                 |
| Concurrency Test   | Multi-agent và lock                 |
| Recovery Test      | Crash, restart và resume            |
| Performance Test   | Session, context, event và indexing |
| Security Test      | Auth, permission, secret và plugin  |
| Chaos Test         | Provider, event bus hoặc DB failure |
| Compatibility Test | Protobuf, event và API version      |

## 31.2. Task Breakdown

| ID      | Task                              |
| ------- | --------------------------------- |
| TEST-01 | Hoàn thiện test strategy          |
| TEST-02 | Xác định coverage target          |
| TEST-03 | Tạo test environment              |
| TEST-04 | Tạo mock provider                 |
| TEST-05 | Tạo Provider SDK contract suite   |
| TEST-06 | Tạo Event Schema contract suite   |
| TEST-07 | Tạo RPC compatibility suite       |
| TEST-08 | Tạo Session recovery suite        |
| TEST-09 | Tạo multi-agent concurrency suite |
| TEST-10 | Tạo workflow recovery suite       |
| TEST-11 | Tạo lock contention suite         |
| TEST-12 | Tạo provider hot-swap suite       |
| TEST-13 | Tạo memory quality benchmark      |
| TEST-14 | Tạo context relevance benchmark   |
| TEST-15 | Tạo large-repository benchmark    |
| TEST-16 | Tạo security test suite           |
| TEST-17 | Tạo chaos test suite              |
| TEST-18 | Tạo release regression suite      |

## 31.3. E2E Scenario bắt buộc

1. CLI tạo session.
2. Planner tạo plan.
3. Architect tạo architecture task.
4. Workflow fan-out cho nhiều Worker.
5. Worker acquire lock.
6. Worker thay đổi workspace.
7. Runtime phát `FileChanged`.
8. Reviewer yêu cầu review.
9. Tester chạy kiểm thử.
10. Merge role đưa thay đổi vào merge queue.
11. Provider được chuyển trong cùng session.
12. Runtime bị restart.
13. Session được resume.
14. IDE reconnect và hiển thị đúng timeline.

---

# 32. Release và GA

## Task Breakdown

| ID     | Task                            |
| ------ | ------------------------------- |
| REL-01 | Định nghĩa versioning policy    |
| REL-02 | Định nghĩa compatibility policy |
| REL-03 | Tạo changelog process           |
| REL-04 | Tạo release pipeline            |
| REL-05 | Tạo artifact signing            |
| REL-06 | Tạo Software Bill of Materials  |
| REL-07 | Tạo installation guide          |
| REL-08 | Tạo configuration guide         |
| REL-09 | Tạo upgrade guide               |
| REL-10 | Tạo migration guide             |
| REL-11 | Tạo troubleshooting guide       |
| REL-12 | Tạo operations runbook          |
| REL-13 | Phát hành Release Candidate     |
| REL-14 | Chạy regression suite           |
| REL-15 | Thực hiện security sign-off     |
| REL-16 | Thực hiện architecture sign-off |
| REL-17 | Thực hiện operation sign-off    |
| REL-18 | Phát hành GA                    |

---

# 33. Milestone đề xuất

Các milestone dưới đây là **đề xuất**, không phải timeline chính thức.

| Milestone | Phạm vi                                       |
| --------- | --------------------------------------------- |
| M0        | Capability Matrix hoàn tất                    |
| M1        | Landscape Research hoàn tất                   |
| M2        | Architecture và ADR được duyệt                |
| M3        | RFC và Production Specification được duyệt    |
| M4        | Runtime, RPC, Event và Storage hoạt động      |
| M5        | Shared Session và Provider hot-swap hoạt động |
| M6        | Agent Runtime và Workflow hoạt động           |
| M7        | Workspace và Locking hoạt động                |
| M8        | Memory và Context hoạt động                   |
| M9        | CLI, TUI, IDE, Plugin và MCP hoạt động        |
| M10       | Production hardening hoàn tất                 |
| M11       | Release Candidate                             |
| M12       | GA                                            |

---

# 34. Ước lượng giai đoạn đề xuất

Ước lượng này chỉ dùng cho planning ban đầu.

| Giai đoạn              |   Ước lượng |
| ---------------------- | ----------: |
| Phase -1               |  1–2 Sprint |
| Phase 0                |  2–3 Sprint |
| Architecture và ADR    |  2–3 Sprint |
| Phase 1 RFC và Spec    |  3–4 Sprint |
| Phase 2 Implementation | 8–12 Sprint |
| Testing và Hardening   |  3–5 Sprint |
| Release và GA          |  1–2 Sprint |

Một Sprint được giả định là 2 tuần.

Ước lượng phải được cập nhật sau:

- Capability Matrix.
- Landscape Research.
- Reuse decision.
- Architecture Review.
- Xác định số lượng thành viên.

---

# 35. Critical Path

```text
PROJECT.md
→ Capability Matrix
→ Landscape Research
→ Architecture Review
→ ADR
→ RFC
→ Production Specification
→ Runtime Foundation
→ Storage
→ Shared Session
→ Provider SDK
→ Event Infrastructure
→ Agent Runtime
→ Workflow
→ Workspace
→ Locking
→ Context
→ CLI/IDE
→ Production Hardening
→ Release Candidate
→ GA
```

Memory, Git, Plugin và MCP có thể phát triển song song tại một số thời điểm nhưng vẫn phụ thuộc vào Session, Security và API contract.

---

# 36. Definition of Ready

Một implementation task chỉ được đưa vào Sprint khi:

- Có mô tả rõ.
- Có dependency.
- Có RFC hoặc specification liên quan.
- ADR liên quan đã được xử lý.
- Có acceptance criteria.
- Có test scenario.
- Có failure scenario.
- Có observability requirement.
- Có security consideration.
- Không còn architecture blocker.

---

# 37. Definition of Done

Task hoàn thành khi:

- Code đã hoàn tất.
- Code review đã approve.
- Unit test đã pass.
- Contract hoặc integration test liên quan đã pass.
- Acceptance criteria đã đạt.
- Failure handling đã được kiểm tra.
- Logging và metrics đã được bổ sung.
- Security consideration đã được kiểm tra.
- Tài liệu đã cập nhật.
- `PROJECT.md` đã cập nhật nếu trạng thái dự án thay đổi.
- Không còn lỗi Critical hoặc High liên quan trực tiếp.

---

# 38. Sprint đầu tiên đề xuất

## Sprint -1A — Project Foundation

### Sprint Goal

Thiết lập source of truth, cấu trúc tài liệu và framework đánh giá capability.

### Tasks

- P-1-01: Tạo `PROJECT.md`.
- P-1-02: Tạo cấu trúc `docs/`.
- P-1-03: Tạo Research template.
- P-1-04: Tạo ADR template.
- P-1-05: Tạo RFC template.
- P-1-06: Coding Agent capability.
- P-1-07: Provider capability.
- P-1-08: Memory capability.
- P-1-09: Workflow capability.
- P-1-10: Workspace capability.

### Kết quả cần đạt

- Repository có cấu trúc rõ ràng.
- `PROJECT.md` tồn tại và được sử dụng.
- Có template thống nhất.
- Capability Matrix đạt khoảng 40–50%.

---

## Sprint -1B — Capability Matrix Completion

### Sprint Goal

Hoàn tất Capability Matrix và chuẩn bị Phase 0.

### Tasks

- P-1-11 đến P-1-18.
- Xác định danh sách OSS cần nghiên cứu.
- Tạo research ownership matrix.
- Xác định scoring model.
- Review Capability Matrix.

### Kết quả cần đạt

- Capability Matrix được review.
- Research categories được phân công.
- Phase 0 có thể bắt đầu.

---

# 39. Các rủi ro chính

| Rủi ro                                | Mức ảnh hưởng | Biện pháp                               |
| ------------------------------------- | ------------- | --------------------------------------- |
| Phạm vi nền tảng quá lớn              | Cao           | Chia milestone và giữ contract ổn định  |
| Research kéo dài                      | Trung bình    | Time-box và dùng scoring model          |
| Provider khác nhau quá nhiều          | Cao           | Capability model và conformance suite   |
| Session model quá phức tạp            | Cao           | State machine và consistency test       |
| Context quá lớn                       | Cao           | Incremental context và token budget     |
| Agent conflict workspace              | Cao           | Hierarchical lock                       |
| Lock bị treo khi agent chết           | Cao           | Lease, heartbeat và timeout             |
| Event xử lý trùng                     | Cao           | Idempotency và deduplication            |
| Workflow retry gây side effect        | Cao           | Idempotency key và rollback             |
| Memory đưa sai dữ liệu                | Trung bình    | Source, ranking và expiration           |
| OSS ngừng duy trì                     | Cao           | Adapter và replacement strategy         |
| Plugin gây rò rỉ dữ liệu              | Cao           | Permission và isolation                 |
| Chi phí provider không kiểm soát      | Trung bình    | Usage và cost tracking                  |
| SQLite và PostgreSQL không đồng nhất  | Cao           | Storage contract và compatibility tests |
| Local Pub/Sub và NATS chồng chức năng | Cao           | ADR-003                                 |
| Tài liệu lệch implementation          | Cao           | Documentation gate trong CI             |

---

# 40. Trạng thái các quyết định hiện tại

| Nội dung               | Trạng thái                              |
| ---------------------- | --------------------------------------- |
| Rust + Tokio           | Định hướng từ PDD, cần ADR xác nhận     |
| gRPC + Protobuf        | Định hướng từ PDD, cần ADR xác nhận     |
| clap                   | Định hướng từ PDD                       |
| Ratatui                | Định hướng từ PDD                       |
| VS Code Extension      | Định hướng từ PDD                       |
| NATS                   | Định hướng từ PDD, cách sử dụng còn TBD |
| SQLite WAL             | Định hướng local mode, TBD              |
| PostgreSQL             | Định hướng production mode, TBD         |
| tree-sitter            | Định hướng từ PDD                       |
| LSP                    | Định hướng từ PDD                       |
| Tantivy                | Định hướng từ PDD                       |
| Qdrant                 | Định hướng từ PDD                       |
| Custom DAG             | Định hướng từ PDD                       |
| MCP                    | Bắt buộc về nguyên tắc                  |
| Memory provider cụ thể | TBD                                     |
| Provider router cụ thể | TBD                                     |
| Git library            | TBD                                     |
| Reference provider     | TBD                                     |
| SLA, RTO và RPO        | TBD                                     |
| Deployment topology    | TBD                                     |

---

# 41. Kết quả cuối cùng

Hệ thống được xem là đạt mục tiêu khi:

1. Session tồn tại độc lập với IDE hoặc CLI.
2. Runtime là thành phần duy nhất quản lý persistent state.
3. Có thể chuyển provider trong cùng session.
4. Provider switching không làm mất context.
5. Multi-agent hoạt động theo workflow chung.
6. Worker phối hợp qua Shared Session và Pub/Sub đã được phê duyệt.
7. Workspace có hierarchical locking.
8. Agent crash có thể recovery hoặc handover.
9. Workflow hỗ trợ retry, fallback, approval và rollback.
10. Context được xây dựng động, không broadcast full history.
11. Memory có source, lifecycle và permission.
12. Git state được liên kết với session.
13. Plugin và MCP được kiểm soát quyền.
14. CLI và IDE chỉ là client.
15. Runtime có logging, metrics, tracing và audit.
16. Có kiểm thử recovery, concurrency, security và performance.
17. Có quy trình deploy, upgrade, backup, restore và rollback.
18. Release đạt Production Specification đã được phê duyệt.
