export interface PolicyRuntimeCapability {
  supported: boolean
}

export function canEnablePolicyProxy(
  capability: PolicyRuntimeCapability | null | undefined,
): boolean {
  return capability?.supported !== false
}
