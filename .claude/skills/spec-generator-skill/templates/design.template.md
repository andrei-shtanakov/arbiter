# Design Specification

> Architecture, APIs, data schemas, and key decisions for {{PROJECT_NAME}}

## 1. Architecture Overview

### 1.1 Principles

| Principle | Description |
|-----------|-------------|
| {{PRINCIPLE_1}} | {{DESCRIPTION_1}} |
| {{PRINCIPLE_2}} | {{DESCRIPTION_2}} |
| {{PRINCIPLE_3}} | {{DESCRIPTION_3}} |

### 1.2 High-Level Diagram

```
┌─────────────────────────────────────────────────────┐
│                    {{SYSTEM_NAME}}                   │
├─────────────────────────────────────────────────────┤
│                                                      │
│   ┌─────────┐    ┌─────────┐    ┌─────────┐        │
│   │ Layer 1 │───►│ Layer 2 │───►│ Layer 3 │        │
│   └─────────┘    └─────────┘    └─────────┘        │
│                                                      │
└─────────────────────────────────────────────────────┘
```

**Traces to:** [REQ-XXX]

---

## 2. Components

### DESIGN-001: {{COMPONENT_NAME}}

#### Description
{{COMPONENT_DESCRIPTION}}

#### Interface
```python
class {{ComponentName}}(ABC):
    @abstractmethod
    def {{method_name}}(self, {{param}}: {{Type}}) -> {{ReturnType}}:
        """{{Description}}"""
        pass
```

#### Configuration
```yaml
{{component_name}}:
  {{option}}: {{value}}
```

**Traces to:** [REQ-XXX]

---

### DESIGN-002: {{COMPONENT_NAME}}

#### Description
{{COMPONENT_DESCRIPTION}}

#### Data Model
```python
@dataclass
class {{ModelName}}:
    {{field_1}}: {{type_1}}
    {{field_2}}: {{type_2}}
    {{field_3}}: {{type_3}} = {{default}}
```

#### API
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | /api/{{resource}} | {{description}} |
| POST | /api/{{resource}} | {{description}} |

**Traces to:** [REQ-XXX]

---

## 3. Data Schemas

### 3.1 {{ENTITY_NAME}}

```json
{
  "{{field_1}}": "{{type}} ({{constraints}})",
  "{{field_2}}": "{{type}} ({{constraints}})",
  "{{nested}}": {
    "{{field_3}}": "{{type}}"
  }
}
```

### 3.2 Database Schema

```sql
CREATE TABLE {{table_name}} (
    id UUID PRIMARY KEY,
    {{column_1}} {{TYPE}} NOT NULL,
    {{column_2}} {{TYPE}},
    created_at TIMESTAMP DEFAULT NOW()
);
```

---

## 4. Integrations

### 4.1 {{INTEGRATION_NAME}}

| Aspect | Value |
|--------|-------|
| Protocol | {{protocol}} |
| Endpoint | {{endpoint}} |
| Authentication | {{auth_method}} |

#### Request Example
```json
{
  "{{field}}": "{{value}}"
}
```

#### Response Example
```json
{
  "{{field}}": "{{value}}"
}
```

**Traces to:** [REQ-XXX]

---

## 5. Key Decisions (ADR)

### ADR-001: {{DECISION_TITLE}}
**Status:** Accepted | Proposed | Deprecated
**Date:** {{DATE}}

**Context:**
{{CONTEXT_DESCRIPTION}}

**Decision:**
{{DECISION_DESCRIPTION}}

**Rationale:**
{{RATIONALE}}

**Consequences:**
- (+) {{POSITIVE_1}}
- (+) {{POSITIVE_2}}
- (-) {{NEGATIVE_1}}

**Traces to:** [REQ-XXX]

---

### ADR-002: {{DECISION_TITLE}}
...

---

## 6. Data Flow

### 6.1 {{FLOW_NAME}}

```
{{INPUT}}
    │
    ▼
┌─────────┐     ┌─────────┐     ┌─────────┐
│ Step 1  │────►│ Step 2  │────►│ Step 3  │
└─────────┘     └─────────┘     └─────────┘
                                     │
                                     ▼
                                {{OUTPUT}}
```

---

## 7. Security Model

### 7.1 Authentication
{{AUTH_DESCRIPTION}}

### 7.2 Authorization
| Role | Permissions |
|------|-------------|
| {{role_1}} | {{permissions}} |
| {{role_2}} | {{permissions}} |

### 7.3 Data Protection
- {{PROTECTION_1}}
- {{PROTECTION_2}}

---

## 8. API Reference

### 8.1 CLI Commands

```bash
# {{COMMAND_1_DESCRIPTION}}
{{command}} {{subcommand}} --{{option}}={{value}}

# {{COMMAND_2_DESCRIPTION}}
{{command}} {{subcommand}} --{{option}}
```

### 8.2 Configuration File

```yaml
# {{config_file_name}}
version: "1.0"

{{section_1}}:
  {{option_1}}: {{value}}
  {{option_2}}: {{value}}

{{section_2}}:
  {{option}}: {{value}}
```

---

## 9. Directory Structure

```
{{project_name}}/
├── {{dir_1}}/
│   ├── {{file_1}}.{{ext}}
│   └── {{file_2}}.{{ext}}
├── {{dir_2}}/
│   ├── {{subdir}}/
│   │   └── {{file}}.{{ext}}
│   └── {{file}}.{{ext}}
├── {{config_file}}
└── README.md
```
