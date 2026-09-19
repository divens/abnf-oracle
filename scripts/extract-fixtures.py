#!/usr/bin/env python3
"""Regenerate tests/grammars/*.abnf from the RFCs they are transcribed from.

The fixtures are the foundation every later test stands on, so they are derived mechanically
rather than typed: one that had quietly drifted from the RFC it claims to be would make every
test depending on it meaningless (PLAN.md R4).

    python scripts/extract-fixtures.py [output-dir] [rfc-cache-dir]

Fetches each RFC from rfc-editor.org (cached on disk), strips page furniture, lifts out the
ABNF blocks, and writes one fixture per grammar with a header comment naming its source and
the errata applied.

Three things in RFC plain text break naive extraction, and each cost a debugging round:

  * A page break can fall in the middle of a rule -- RFC 5322 splits obs-zone across one -- so
    the blank lines around a page boundary are removed along with the footer and the running
    header, closing the rule back up.
  * One document can print its grammar at different indents in different sections (RFC 8259),
    so each block is dedented by its own base indent, never by a global minimum. Getting this
    wrong does not raise an error: a rule left at the wrong indent parses as a *continuation*
    of the rule above it.
  * Prose indented deeper than the grammar above it looks like a continuation (RFC 3339's NOTE
    paragraphs), so a blank line ends a block unless a sibling rule definition follows.
"""
import io
import os
import re
import sys
import urllib.request

RULE = re.compile(r'^(\s*)([A-Za-z][A-Za-z0-9-]*)\s*=(/?)(\s|$)')
FOOTER = re.compile(r'\[Page \d+\]\s*$')
HEADER = re.compile(r'^RFC \d+\s')


def load(path):
    """Read an RFC, stripping page furniture and closing up rules a page break split."""
    text = io.open(path, encoding='utf-8', errors='replace').read()
    lines = [l.rstrip() for l in text.split('\n')]

    def furniture(l):
        return '\f' in l or FOOTER.search(l) or HEADER.match(l)

    keep, i = [], 0
    while i < len(lines):
        if furniture(lines[i]):
            while keep and not keep[-1].strip():
                keep.pop()
            while i < len(lines) and (furniture(lines[i]) or not lines[i].strip()):
                i += 1
            continue
        keep.append(lines[i])
        i += 1
    return keep


def section(lines, start_re, end_re):
    """The lines from the heading matching start_re up to the one matching end_re."""
    start = next(i for i, l in enumerate(lines) if re.match(start_re, l))
    end = next(i for i, l in enumerate(lines[start + 1:], start + 1) if re.match(end_re, l))
    return lines[start:end]


def blocks(lines):
    """Lift out the grammar blocks, each dedented by its own base indent.

    A block starts at a rule definition and continues through deeper-indented continuation
    lines and sibling rules at the same indent. A blank line ends it unless a sibling rule
    follows: deeper text after a blank line is prose, not a continuation.
    """
    out, i = [], 0
    while i < len(lines):
        match = RULE.match(lines[i])
        if not match:
            i += 1
            continue
        base = len(match.group(1))
        block, j = [], i
        while j < len(lines):
            line = lines[j]
            if not line.strip():
                k = j
                while k < len(lines) and not lines[k].strip():
                    k += 1
                sibling = (k < len(lines) and RULE.match(lines[k])
                           and len(lines[k]) - len(lines[k].lstrip()) == base)
                if sibling:
                    block.extend('' for _ in lines[j:k])
                    j = k
                    continue
                break
            indent = len(line) - len(line.lstrip())
            if indent > base or (indent == base and RULE.match(line)):
                block.append(line[base:])
                j += 1
                continue
            break
        out.extend(block)
        out.append('')
        i = j
    return out


def tidy(lines):
    """Collapse runs of blank lines and drop trailing ones."""
    out = []
    for line in lines:
        if not line.strip() and (not out or not out[-1].strip()):
            continue
        out.append(line.rstrip())
    while out and not out[-1].strip():
        out.pop()
    return out


def emit(path, header, lines):
    body = tidy(lines)
    text = ''.join('; ' + h + '\n' for h in header) + '\n' + '\n'.join(body) + '\n'
    io.open(path, 'w', encoding='utf-8', newline='\n').write(text)
    rules = sum(1 for l in body if RULE.match(l))
    print('{:<40} {:>4} lines, {:>3} rules'.format(os.path.basename(path), len(body), rules))


def fetch(number, cache):
    path = os.path.join(cache, 'rfc{}.txt'.format(number))
    if not os.path.exists(path):
        url = 'https://www.rfc-editor.org/rfc/rfc{}.txt'.format(number)
        print('fetching ' + url)
        with urllib.request.urlopen(url) as response:
            body = response.read().decode('utf-8', errors='replace')
        io.open(path, 'w', encoding='utf-8', newline='\n').write(body)
    return path


# The two verified errata on RFC 5234. Both correct the grammar in section 4; neither has
# anything to do with numeric values (SCOPE.md D39).
E2968 = ('elements       =  alternation *c-wsp', 'elements       =  alternation *WSP')
E3076 = ('rulelist       =  1*( rule / (*c-wsp c-nl) )',
         'rulelist       =  1*( rule / (*WSP c-nl) )')


def build(out, cache):
    os.makedirs(out, exist_ok=True)
    os.makedirs(cache, exist_ok=True)

    def target(name):
        return os.path.join(out, name)

    rfc5234 = load(fetch(5234, cache))

    emit(target('rfc5234-core.abnf'),
         ['RFC 5234 Appendix B.1 - Core Rules',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc5234.txt',
          'Errata applied: none (no verified erratum touches Appendix B)'],
         blocks(section(rfc5234, r'^B\.1\.\s+Core Rules', r'^B\.2\.')))

    published = tidy(blocks(
        section(rfc5234, r'^4\.\s+ABNF Definition of ABNF', r'^5\.\s+Security')))
    emit(target('rfc5234-abnf-as-published.abnf'),
         ['RFC 5234 Section 4 - ABNF Definition of ABNF, exactly as published',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc5234.txt',
          'Errata applied: none. This variant is ambiguous; see Errata 2968 and 3076.',
          'Kept as a parse fixture only - it is NOT the grammar this crate implements.'],
         published)

    errata, seen = [], set()
    for line in published:
        for old, new in (E2968, E3076):
            if line.strip() == old.strip():
                line = line.replace(old.strip(), new.strip())
                seen.add(old)
        errata.append(line)
    missing = {E2968[0], E3076[0]} - seen
    assert not missing, 'erratum target not found: {}'.format(missing)

    emit(target('rfc5234-abnf-errata.abnf'),
         ['RFC 5234 Section 4 - ABNF Definition of ABNF, with both verified errata',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc5234.txt',
          'Errata applied: 2968 (elements = alternation *WSP)',
          '                3076 (rulelist = 1*( rule / (*WSP c-nl) ))',
          'Both remove an ambiguity in the grammar; neither changes its language.'],
         errata)

    # RFC 7405 replaces char-val outright, so splice its block in where that rule was.
    amend = tidy(blocks(section(load(fetch(7405, cache)), r'^2\.2\.', r'^3\.\s')))
    assert any(l.startswith('char-val') for l in amend)
    assert any(l.startswith('quoted-string') for l in amend)

    canonical, i, spliced = [], 0, False
    while i < len(errata):
        if errata[i].startswith('char-val'):
            i += 1
            while i < len(errata) and errata[i].strip() and errata[i].lstrip().startswith(';'):
                i += 1
            canonical.extend(amend)
            spliced = True
            continue
        canonical.append(errata[i])
        i += 1
    assert spliced, 'char-val not found in section 4'

    emit(target('abnf-canonical.abnf'),
         ['The canonical self-grammar: the ABNF this crate implements.',
          'RFC 5234 Section 4 + Errata 2968 and 3076 + RFC 7405 Section 2.2.',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc5234.txt',
          '                 https://www.rfc-editor.org/rfc/rfc7405.txt',
          'Errata applied: 2968 (elements = alternation *WSP)',
          '                3076 (rulelist = 1*( rule / (*WSP c-nl) ))',
          'RFC 7405 replaces char-val with the case-sensitive string forms.'],
         canonical)

    emit(target('rfc8259-json.abnf'),
         ['RFC 8259 Sections 2-8 - The JavaScript Object Notation (JSON) Data Interchange Format',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc8259.txt',
          'Errata applied: none',
          'Note: this grammar defines `char`, which shadows the core rule CHAR.'],
         blocks(section(load(fetch(8259, cache)), r'^2\.\s+JSON Grammar', r'^9\.\s')))

    emit(target('rfc3986-uri.abnf'),
         ['RFC 3986 Appendix A - Collected ABNF for URI',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc3986.txt',
          'Errata applied: none'],
         blocks(section(load(fetch(3986, cache)), r'^Appendix A\.', r'^Appendix B\.')))

    emit(target('rfc3339-datetime.abnf'),
         ['RFC 3339 Section 5.6 - Internet Date/Time Format',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc3339.txt',
          'Errata applied: none',
          "Appendix A's ISO 8601 grammar is deliberately excluded: it redefines",
          'several of these rule names, which would be a duplicate definition.'],
         blocks(section(load(fetch(3339, cache)), r'^5\.6\.', r'^5\.7\.|^6\.\s')))

    emit(target('rfc5322-message.abnf'),
         ['RFC 5322 Sections 3 and 4 - Internet Message Format',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc5322.txt',
          'Errata applied: none',
          'Section 4 (Obsolete Syntax) is included because section 3 references its',
          'obs-* rules; without it the grammar would have undefined references.'],
         blocks(section(load(fetch(5322, cache)), r'^3\.\s+Syntax', r'^5\.\s+Security')))

    emit(target('rfc9110-http.abnf'),
         ['RFC 9110 Appendix A - Collected ABNF (HTTP Semantics)',
          'Transcribed from https://www.rfc-editor.org/rfc/rfc9110.txt',
          'Errata applied: none',
          'Exercises octet-range terminals (obs-text = %x80-FF, see SCOPE.md 3) and',
          'prose values: the URI rules are given as <...> references to RFC 3986.'],
         blocks(section(load(fetch(9110, cache)), r'^Appendix A\.', r'^Appendix B\.|^Index')))


if __name__ == '__main__':
    build(sys.argv[1] if len(sys.argv) > 1 else 'tests/grammars',
          sys.argv[2] if len(sys.argv) > 2 else '.rfc-cache')
