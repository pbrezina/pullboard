import { compileSprintNamePattern } from './sprint-name-pattern';

describe('sprint name patterns', () => {
  const pattern = compileSprintNamePattern('%YEAR-IDM-SSSD-IDENTITY-S%NUMBER');

  it.each(['2026-IDM-SSSD-IDENTITY-S23', '2027-IDM-SSSD-IDENTITY-S01'])(
    'matches %s without pinning the current year or sprint number',
    (name) => expect(pattern.test(name)).toBe(true),
  );

  it.each([
    '26-IDM-SSSD-IDENTITY-S23',
    '20260-IDM-SSSD-IDENTITY-S23',
    '2026-IDM-SSSD-IDENTITY-S',
    '2026-IDM-SSSD-IDENTITY-Sabc',
    '2026-IDM-SSSD-IDENTITY-S2.5',
    '2026-IDM-SSSD-OTHER-S23',
    '2026-idm-sssd-identity-S23',
    'prefix-2026-IDM-SSSD-IDENTITY-S23',
    '2026-IDM-SSSD-IDENTITY-S23-suffix',
  ])('does not match %s', (name) => expect(pattern.test(name)).toBe(false));

  it('treats regular expression characters literally', () => {
    const literal = compileSprintNamePattern('%YEAR-Team.[A]+(B)-S%NUMBER');
    expect(literal.test('2026-Team.[A]+(B)-S12')).toBe(true);
    expect(literal.test('2026-TeamxAAAB-S12')).toBe(false);
  });

  it('also supports an exact name without placeholders', () => {
    expect(compileSprintNamePattern('Team Sprint').test('Team Sprint')).toBe(true);
    expect(compileSprintNamePattern('Team Sprint').test('Team Sprint 1')).toBe(false);
  });

  it('rejects unknown placeholders', () => {
    expect(() => compileSprintNamePattern('%YEAR-Team-%SPRINT')).toThrow(
      'supports only %YEAR and %NUMBER',
    );
  });
});
