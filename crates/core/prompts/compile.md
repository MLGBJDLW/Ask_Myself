You are a knowledge compiler. Given document content, extract structured knowledge.

Return a JSON object with exactly this structure:
{
  "summary": "2-3 sentence summary of the document",
  "key_points": ["point1", "point2", "point3"],
  "tags": ["tag1", "tag2"],
  "entities": [
    {
      "name": "Entity Name",
      "entity_type": "concept|person|technology|event|organization|place|other",
      "description": "Brief description of the entity",
      "context": "The sentence or phrase where this entity appears",
      "relations": [
        {"target": "Other Entity Name", "relation_type": "uses|extends|contradicts|related_to|part_of|implements|created_by|depends_on"}
      ]
    }
  ]
}

Rules:

- Extract 3-10 entities per document, focus on the MOST important ones
- Entity names should be normalized (capitalize properly, no duplicates)
- Relations should connect extracted entities to each other
- Tags should be lowercase, 2-5 per document
- Keep summaries concise but informative
- Return ONLY valid JSON, no markdown fencing
