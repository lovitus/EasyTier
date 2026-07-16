export interface PolicyRuntimeCapability {
  supported: boolean
  platform?: string
}

export type PolicyRuntimeNotice =
  | 'linux-supported'
  | 'android-experimental'
  | 'macos-partial'
  | 'windows-unsupported'
  | 'supported'
  | 'unsupported'

export function canEnablePolicyProxy(
  capability: PolicyRuntimeCapability | null | undefined,
): boolean {
  return capability?.supported !== false
}

export function policyRuntimeNotice(
  capability: PolicyRuntimeCapability,
): PolicyRuntimeNotice {
  const platform = capability.platform?.trim().toLowerCase() ?? ''
  if (platform === 'android') {
    return capability.supported ? 'android-experimental' : 'unsupported'
  }
  if (platform === 'darwin' || platform === 'macos' || platform === 'mac') {
    return 'macos-partial'
  }
  if (platform === 'windows' || platform === 'win32') {
    return 'windows-unsupported'
  }
  if (!capability.supported) return 'unsupported'
  if (platform === 'linux') return 'linux-supported'
  return 'supported'
}
