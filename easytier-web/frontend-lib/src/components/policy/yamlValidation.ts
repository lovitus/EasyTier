import { parseDocument } from 'yaml'

export interface YamlSyntaxDiagnostic {
  from: number
  to: number
  message: string
}

export function yamlSyntaxDiagnostics(source: string): YamlSyntaxDiagnostic[] {
  const document = parseDocument(source, {
    prettyErrors: true,
    strict: true,
    uniqueKeys: true,
  })

  return document.errors.map((error) => {
    const [start = 0, end = start + 1] = error.pos ?? []
    const from = Math.min(start, source.length)
    return {
      from,
      to: Math.min(source.length, Math.max(from + 1, end)),
      message: error.message,
    }
  })
}
