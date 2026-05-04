//! Built-in visible personas for shaping assistant behavior.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub instructions: &'static str,
}

pub fn builtin_personas() -> &'static [PersonaProfile] {
    &[
        PersonaProfile {
            id: "default",
            name: "Default",
            description: "Balanced workspace assistant.",
            instructions: "",
        },
        PersonaProfile {
            id: "novelist",
            name: "Novelist",
            description: "Long-form fiction planning, prose, scene craft, continuity.",
            instructions: "Emphasize story structure, character motivation, sensory detail, continuity, and prose quality. When drafting fiction, preserve the user's canon and project memory before inventing new details.",
        },
        PersonaProfile {
            id: "speaker",
            name: "Speaker",
            description: "Speechwriter and presentation coach.",
            instructions: "Emphasize audience, spoken rhythm, persuasive structure, memorable phrasing, timing, and delivery notes. Prefer language that sounds natural when read aloud.",
        },
        PersonaProfile {
            id: "researcher",
            name: "Researcher",
            description: "Evidence-heavy research and synthesis.",
            instructions: "Emphasize source quality, uncertainty, citations, comparison, and gaps. Separate evidence from inference clearly.",
        },
        PersonaProfile {
            id: "editor",
            name: "Editor",
            description: "Revision, style, and clarity.",
            instructions: "Emphasize concise editing, structure, tone consistency, grammar, and preservation of the author's intent. Explain major edits briefly.",
        },
    ]
}

pub fn persona_by_id(id: &str) -> Option<&'static PersonaProfile> {
    builtin_personas().iter().find(|persona| persona.id == id)
}

pub fn build_persona_prompt_section(persona_id: Option<&str>) -> String {
    let Some(id) = persona_id else {
        return String::new();
    };
    let Some(persona) = persona_by_id(id) else {
        return String::new();
    };
    if persona.id == "default" || persona.instructions.trim().is_empty() {
        return String::new();
    }
    format!(
        "## Active Persona\n\nPersona: {} ({})\n\nInstructions: {}\n\nPersona instructions shape voice and workflow emphasis only. They do not override system, user, evidence, privacy, source-scope, or tool rules.",
        persona.name, persona.description, persona.instructions
    )
}
