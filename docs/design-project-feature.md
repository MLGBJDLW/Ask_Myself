# Design: Project (Workspace) Feature

**Status:** Proposed  
**Date:** 2026-04-13  
**Author:** Ouroboros Architect  

---

## 1. Context & Problem

Conversations are currently a flat, time-sorted list. As users accumulate dozens of conversations, finding related ones becomes difficult. The product direction calls for "collection-as-workspace behavior" (Pillar 5) and consumer-grade usability (Pillar 4). A lightweight "Project" concept groups conversations and optionally playbooks under a named space with shared context.

### Constraints

| Constraint | Detail |
|---|---|
| SQLite-only | No new DB dependencies |
| Additive schema | No breaking changes to existing tables |
| Consumer-friendly | Non-technical users must understand "projects" |
| Collections | Must integrate naturally with existing Playbooks |
| Backward compat | Existing conversations remain "ungrouped" |

---

## 2. Database Schema

### 2.1 New `projects` Table

```sql
-- Migration: v038_projects
CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    icon        TEXT NOT NULL DEFAULT '',         -- emoji or icon key
    color       TEXT NOT NULL DEFAULT '',         -- CSS color for sidebar badge
    system_prompt TEXT NOT NULL DEFAULT '',       -- shared project-level system prompt
    source_scope_json TEXT,                       -- shared default source IDs (JSON array)
    archived    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
CREATE INDEX IF NOT EXISTS idx_projects_archived ON projects(archived);
```

### 2.2 Add `project_id` to `conversations`

```sql
-- Migration: v039_conversation_project
ALTER TABLE conversations ADD COLUMN project_id TEXT
    REFERENCES projects(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_conversations_project
    ON conversations(project_id);
```

`ON DELETE SET NULL` — if a project is deleted, its conversations revert to ungrouped instead of being cascade-deleted. This is safer and matches user expectations.

### 2.3 Add `project_id` to `playbooks`

```sql
-- Migration: v040_playbook_project
ALTER TABLE playbooks ADD COLUMN project_id TEXT
    REFERENCES projects(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_playbooks_project
    ON playbooks(project_id);
```

### 2.4 Migration Strategy

| Step | Detail |
|---|---|
| Fresh install | `v_initial_consolidated.sql` stays unchanged for now; v038–v040 run as incremental migrations |
| Existing users | Conversations get `project_id = NULL` (ungrouped). Zero data loss. |
| Future consolidation | Fold into consolidated schema when next major consolidation happens |

**Why This Design:** Additive-only `ALTER TABLE ADD COLUMN` is the safest SQLite migration pattern. `ON DELETE SET NULL` prevents accidental data loss. NULLable FK means ungrouped conversations need zero migration.

---

## 3. Data Model (Rust)

### 3.1 `Project` Struct

```rust
// crates/core/src/project.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: String,
    pub system_prompt: String,
    pub source_scope: Option<Vec<String>>,  // deserialized from source_scope_json
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInput {
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub system_prompt: Option<String>,
    pub source_scope: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub system_prompt: Option<String>,
    pub source_scope: Option<Vec<String>>,
    pub archived: Option<bool>,
}
```

### 3.2 Modified `Conversation` Struct

```rust
// Add to existing Conversation struct in conversation/mod.rs
pub struct Conversation {
    // ... existing fields ...
    pub project_id: Option<String>,  // NEW
}
```

The TS type mirrors this:

```typescript
// types/conversation.ts
export interface Conversation {
  // ... existing fields ...
  projectId?: string | null;
}
```

### 3.3 New TypeScript Type

```typescript
// types/project.ts
export interface Project {
  id: string;
  name: string;
  description: string;
  icon: string;
  color: string;
  systemPrompt: string;
  sourceScope: string[] | null;
  archived: boolean;
  createdAt: string;
  updatedAt: string;
}
```

**Why This Design:** Follows the exact same patterns as existing `Conversation` and `Playbook` structs (camelCase serde, String IDs, datetime as String). `Option<String>` for `project_id` keeps backward compatibility.

---

## 4. Tauri Commands

New commands registered in `commands.rs` and `main.rs`:

| Command | Purpose |
|---|---|
| `create_project_cmd` | Create project, return `Project` |
| `list_projects_cmd` | List all non-archived projects |
| `get_project_cmd(id)` | Get single project |
| `update_project_cmd(id, input)` | Update name, description, prompt, etc. |
| `delete_project_cmd(id)` | Delete project (conversations become ungrouped) |
| `archive_project_cmd(id)` | Soft-archive a project |
| `move_conversation_to_project_cmd(conv_id, project_id)` | Assign/unassign conversation |
| `move_playbook_to_project_cmd(playbook_id, project_id)` | Assign/unassign playbook |
| `list_conversations_cmd` | **Modified**: add optional `project_id` filter param |
| `create_conversation_cmd` | **Modified**: accept optional `project_id` |

**Why This Design:** Follows existing Tauri command naming convention (`_cmd` suffix, `state: tauri::State`). Move operations are explicit rather than implicit — user drags or picks from a menu.

---

## 5. UI Architecture

### 5.1 Sidebar Integration (`ChatSidebar.tsx`)

```
┌──────────────────────────────┐
│ 🔍 Search conversations      │
│ ➕ New Chat                   │
├──────────────────────────────┤
│ 📁 All Conversations    ▾    │  ← Project switcher dropdown
├──────────────────────────────┤
│ ★ Pinned                     │
│   ├ Conv A                   │
│ Today                        │
│   ├ Conv B                   │
│   ├ Conv C                   │
│ Yesterday                    │
│   ├ Conv D                   │
└──────────────────────────────┘
```

**Project switcher** replaces the static sidebar header with a dropdown:

| State | Behavior |
|---|---|
| "All Conversations" (default) | Shows all ungrouped + all projects' conversations (current behavior) |
| Specific project selected | Filters sidebar to that project's conversations only |
| New conversation created | Inherits `project_id` from active project selection, or NULL if "All" |

### 5.2 Project Switcher Component

```
┌─────────────────────────────────────┐
│  📁 Research Paper ▾                │
│ ┌─────────────────────────────────┐ │
│ │ ✓ Research Paper                │ │
│ │   Work Project                  │ │
│ │   Personal Notes                │ │
│ │ ─────────────────               │ │
│ │   All Conversations             │ │
│ │ ─────────────────               │ │
│ │ ➕ New Project...                │ │
│ │ ⚙ Manage Projects...            │ │
│ └─────────────────────────────────┘ │
└─────────────────────────────────────┘
```

Implementation approach:
- New `ProjectSwitcher` component (dropdown/popover)
- Placed above the search bar in `ChatSidebar`
- Stores active project in `localStorage` or React context (survives page nav)
- When a project is active, `list_conversations_cmd` filters by `project_id`

### 5.3 Project Creation Flow

1. User clicks "➕ New Project..." in switcher
2. Inline dialog or modal with fields: **Name** (required), **Description** (optional), **Icon** (emoji picker), **Color** (preset palette)
3. Optional: set project-level system prompt and source scope
4. On create → project becomes active in switcher

### 5.4 Moving Conversations Between Projects

Two methods:
1. **Right-click context menu** on conversation item → "Move to Project" → submenu of projects
2. **Drag-and-drop** (future phase)

### 5.5 Project Settings Page

Accessible via "⚙ Manage Projects..." or project context menu:
- Edit name, description, icon, color
- Set project-level system prompt (inherited by new conversations)
- Set default source scope (inherited by new conversations)
- View conversation/playbook count
- Archive/delete project

### 5.6 Playbooks Page Integration

The existing `/playbooks` page gets a project filter:
- Dropdown matching the same `ProjectSwitcher` state
- When a project is active, only that project's playbooks show
- "All" shows everything (current behavior)

**Why This Design:** The switcher pattern is familiar from tools like Notion, Linear, and Slack. It doesn't break the existing time-grouped sidebar — it layers a filter on top. Non-technical users understand "folders" or "workspaces" intuitively.

---

## 6. Shared Context Inheritance

When a project has `system_prompt` and/or `source_scope` set:

| Scenario | Behavior |
|---|---|
| New conversation in project | Inherits project's `system_prompt` and `source_scope` as defaults |
| Conversation overrides prompt | Conversation's own `system_prompt` takes precedence |
| Conversation has no override | Falls back to project-level prompt |
| Project prompt changes | Only affects future conversations (existing ones keep their snapshot) |

### Resolution Logic (Rust)

```rust
fn effective_system_prompt(conv: &Conversation, project: Option<&Project>) -> String {
    if !conv.system_prompt.is_empty() {
        conv.system_prompt.clone()
    } else if let Some(p) = project {
        if !p.system_prompt.is_empty() {
            p.system_prompt.clone()
        } else {
            String::new()
        }
    } else {
        String::new()
    }
}
```

**Why This Design:** "Snapshot on create" was considered but rejected — it creates invisible divergence. "Live inheritance with override" is more intuitive: project prompt is a default, conversation prompt is a customization. If the user edits the conversation's prompt, it sticks. If not, they get the project's.

---

## 7. Alternatives Considered

### ALT-001: Tags Instead of Projects

- **Description:** Add a `tags` JSON column to conversations; group by tag in sidebar.
- **Rejected because:** Tags are many-to-many and harder to reason about for non-technical users. A conversation belonging to multiple "projects" is confusing. Tags also can't carry shared context (system prompt, source scope).

### ALT-002: Extend Collections (Playbooks) to Hold Conversations

- **Description:** Add a `conversations` join table to playbooks, making collections the grouping mechanism.
- **Rejected because:** Playbooks have a specific evidence-collection purpose (citations, chunks, query_text). Overloading them as workspace containers conflates two concepts. Product direction says collections should evolve into "investigation packs", not project folders.

### ALT-003: Hierarchical Folders (Nested)

- **Description:** Full folder tree with arbitrary nesting depth.
- **Rejected because:** Over-engineered for v1. Adds schema complexity (parent_id, path). Consumer users struggle with deep hierarchies. Can be added later if flat projects prove insufficient.

| Alternative | Simplicity | Shared Context | Consumer UX | Future-proof |
|---|---|---|---|---|
| **Projects (chosen)** | ✅ | ✅ | ✅ | ✅ |
| Tags | ✅ | ❌ | 🟡 | 🟡 |
| Extend Collections | 🟡 | 🟡 | ❌ | ❌ |
| Nested Folders | ❌ | ✅ | ❌ | ✅ |

---

## 8. Phased Rollout

### Phase 1 — Minimal Viable (v1)

| Item | Detail |
|---|---|
| `projects` table + migrations | v038, v039, v040 |
| `Project` Rust struct + CRUD | New `crates/core/src/project.rs` |
| Tauri commands | 8 new + 2 modified |
| `ProjectSwitcher` component | Dropdown in sidebar |
| `Conversation.projectId` | Display + filter |
| Create/rename/delete project | Basic management |
| Move conversation to project | Context menu |
| Project-level system prompt | Inherited by new conversations |

**Scope boundary:** No drag-and-drop, no playbook integration, no project-level source scope, no archive.

### Phase 2 — Extended

| Item | Detail |
|---|---|
| Playbook project assignment | `playbooks.project_id` + UI filter |
| Project-level source scope | Default sources for new conversations |
| Project archive | Soft-hide completed projects |
| Project settings page | Full edit UI for prompt, sources, metadata |
| Conversation create inherits source scope | From active project |

### Phase 3 — Advanced

| Item | Detail |
|---|---|
| Project-level shared memories | `user_memories` linked to project |
| Drag-and-drop reorder/move | Sidebar DnD |
| Project templates | Pre-configured projects for common workflows |
| Cross-project search | Search across all or specific projects |
| Project export/import | Backup/share a project |

---

## 9. File Impact Summary

| File | Change | Phase |
|---|---|---|
| `crates/core/src/migrations/mod.rs` | Add v038, v039, v040 migration entries | 1 |
| `crates/core/src/project.rs` | **NEW** — Project types + DB CRUD | 1 |
| `crates/core/src/lib.rs` | Add `pub mod project;` | 1 |
| `crates/core/src/conversation/mod.rs` | Add `project_id` to `Conversation`, modify queries | 1 |
| `apps/desktop/src-tauri/src/commands.rs` | Add project commands, modify conversation commands | 1 |
| `apps/desktop/src-tauri/src/main.rs` | Register new commands | 1 |
| `apps/desktop/src/types/project.ts` | **NEW** — TypeScript types | 1 |
| `apps/desktop/src/types/conversation.ts` | Add `projectId` field | 1 |
| `apps/desktop/src/components/chat/ChatSidebar.tsx` | Add `ProjectSwitcher`, filter by project | 1 |
| `apps/desktop/src/components/chat/ProjectSwitcher.tsx` | **NEW** — Dropdown component | 1 |
| `apps/desktop/src/pages/PlaybooksPage.tsx` | Add project filter | 2 |
| `apps/desktop/src/pages/ProjectSettingsPage.tsx` | **NEW** — Project settings | 2 |

---

## 10. Index Recommendations

| Index | Purpose |
|---|---|
| `idx_conversations_project ON conversations(project_id)` | Fast filter by project |
| `idx_playbooks_project ON playbooks(project_id)` | Fast filter by project |
| `idx_projects_name ON projects(name)` | Fast lookup/sort |
| `idx_projects_archived ON projects(archived)` | Filter out archived |

---

## 11. Risks & Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Users don't understand "Projects" | 🟡 | Default to "All Conversations" view. Projects are opt-in. Use clear onboarding tooltip. |
| Too many projects clutters switcher | 🟢 | Archive feature (Phase 2) + search within switcher |
| Project-level prompt conflicts with conversation prompt | 🟡 | Clear "override" semantics. Show indicator when a conversation has custom prompt vs inherited. |
| Migration fails on existing DB | 🟢 | `ALTER TABLE ADD COLUMN` is safe in SQLite. `IF NOT EXISTS` guards on new tables. |
| Performance with many projects | 🟢 | Index on `project_id`. Projects count expected to be < 50 for most users. |

---

## 12. Security Considerations

- **No new attack surface:** Projects are local-only, same as conversations.
- **SQL injection:** All queries use parameterized `?1` placeholders (existing pattern).
- **Data integrity:** `ON DELETE SET NULL` prevents orphaned conversations.
- **No auth changes:** Single-user desktop app, no multi-tenant concerns.

---

## 13. Open Questions

1. **Naming:** "Project" vs "Workspace" vs "Space"? Product decision — "Project" is the simplest for non-technical users.
2. **Default project:** Should there be a "Default" project that catches all ungrouped conversations, or should ungrouped remain NULL? Recommendation: NULL (simpler, no magic defaults).
3. **Project-level memories:** Should `user_memories` gain a `project_id`? Deferred to Phase 3 — requires deeper design around memory isolation vs sharing.
