/** Match the complete sprint name, treating everything except placeholders literally. */
export function compileSprintNamePattern(pattern: string): RegExp {
  const escaped = pattern.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const source = escaped.replace(/%[A-Za-z_]+/g, (token) => {
    switch (token) {
      case '%YEAR':
        return '[0-9]{4}';
      case '%NUMBER':
        return '[0-9]+';
      default:
        throw new Error('jira.sprintNamePattern supports only %YEAR and %NUMBER placeholders.');
    }
  });
  return new RegExp(`^${source}$`);
}
