export interface MihomoListenerStatus {
  bind_address?: string
  mixed_port?: number
  http_port?: number
  socks_port?: number
}

function formatListenEndpoint(bindAddress: string | undefined, port: number): string {
  const address = bindAddress?.trim()
  if (!address) return String(port)
  const host = address.includes(':') && !address.startsWith('[')
    ? `[${address}]`
    : address
  return `${host}:${port}`
}

export function formatMihomoListenPorts(status?: MihomoListenerStatus): string {
  if (!status) return ''
  return [
    status.mixed_port ? `mixed ${formatListenEndpoint(status.bind_address, status.mixed_port)}` : '',
    status.http_port ? `http ${formatListenEndpoint(status.bind_address, status.http_port)}` : '',
    status.socks_port ? `socks ${formatListenEndpoint(status.bind_address, status.socks_port)}` : '',
  ].filter(Boolean).join(' · ')
}
