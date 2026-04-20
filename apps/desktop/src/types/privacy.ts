export interface RedactRule {
  name: string;
  pattern: string;
  replacement: string;
}

export interface PrivacyConfig {
  excludePatterns: string[];
  redactPatterns: RedactRule[];
  enabled: boolean;
}
