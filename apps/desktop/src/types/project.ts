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

export interface CreateProjectInput {
  name: string;
  description?: string | null;
  icon?: string | null;
  color?: string | null;
  systemPrompt?: string | null;
  sourceScope?: string[] | null;
}

export interface UpdateProjectInput {
  name?: string | null;
  description?: string | null;
  icon?: string | null;
  color?: string | null;
  systemPrompt?: string | null;
  sourceScope?: string[] | null;
  archived?: boolean | null;
}
