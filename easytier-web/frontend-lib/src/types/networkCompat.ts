import {
  Action as AclAction,
  ChainType as AclChainType,
  Protocol as AclProtocol,
} from '../generated/proto/acl'
import type { NetworkConfig } from './network'

const UINT64_MAX = (1n << 64n) - 1n

type JsonRecord = Record<string, unknown>

export type PolicyProxyBackend = 'off' | 'mihomo' | 'leaf'

export function policyBackendSupported(
  backend: PolicyProxyBackend,
  platform?: string,
  leafRuntimeSupported = true,
): boolean {
  const normalizedPlatform = platform?.trim().toLowerCase()
  const mobilePlatform =
    normalizedPlatform === 'android' ||
    normalizedPlatform === 'ios' ||
    normalizedPlatform === 'ohos'
  if (backend === 'mihomo') return !mobilePlatform
  if (backend === 'leaf') return leafRuntimeSupported
  return true
}

export function configuredPolicyBackend(
  config: Pick<NetworkConfig, 'enable_policy_proxy' | 'policy_proxy_backend'>,
): PolicyProxyBackend {
  if (
    config.policy_proxy_backend === 'off' ||
    config.policy_proxy_backend === 'mihomo' ||
    config.policy_proxy_backend === 'leaf'
  ) {
    return config.policy_proxy_backend
  }
  return config.enable_policy_proxy ? 'leaf' : 'off'
}

export function applyPolicyBackend(
  config: NetworkConfig,
  backend: PolicyProxyBackend,
): void {
  const previousBackend = configuredPolicyBackend(config)
  if (
    previousBackend === 'mihomo'
    && !config.policy_mihomo_config_file?.trim()
    && !config.policy_mihomo_config_inline?.trim()
  ) {
    config.policy_mihomo_config_file = config.policy_config_file
    config.policy_mihomo_config_inline = config.policy_config_inline
    config.policy_config_file = ''
    config.policy_config_inline = ''
  }
  config.policy_proxy_backend = backend
  // The legacy boolean selects Leaf only. Explicit Mihomo must leave it false
  // or Core correctly rejects the envelope as conflicting backend ownership.
  config.enable_policy_proxy = backend === 'leaf'

}

export function normalizePolicyBackendConfig(config: NetworkConfig): NetworkConfig {
  const normalized = { ...config }
  applyPolicyBackend(normalized, configuredPolicyBackend(config))
  return normalized
}

export function prepareNetworkConfigForProtoJson(config: NetworkConfig): NetworkConfig {
  const prepared = dropUnsupportedJsonValues(
    applyLegacyAclDefaults(normalizePolicyBackendConfig(config)),
  ) as NetworkConfig
  normalizeLegacyOptionalUint64(prepared as JsonRecord, 'instance_recv_bps_limit')
  return prepared
}

function applyLegacyAclDefaults(config: NetworkConfig): NetworkConfig {
  const acl = config.acl
  const aclV1 = acl?.acl_v1
  if (!Array.isArray(aclV1?.chains)) return config

  return {
    ...config,
    acl: {
      ...acl,
      acl_v1: {
        ...aclV1,
        chains: aclV1.chains.map((chain) => ({
          ...chain,
          chain_type: chain.chain_type ?? AclChainType.UnspecifiedChain,
          default_action: chain.default_action ?? AclAction.Allow,
          rules: (chain.rules ?? []).map((rule) => ({
            ...rule,
            protocol: rule.protocol ?? AclProtocol.Any,
            action: rule.action ?? AclAction.Allow,
          })),
        })),
      },
    },
  }
}

function dropUnsupportedJsonValues(value: unknown): unknown {
  if (value === undefined) return undefined
  if (typeof value === 'number' && !Number.isFinite(value)) return undefined

  if (Array.isArray(value)) {
    return value.map(dropUnsupportedJsonValues).filter((v) => v !== undefined)
  }

  if (isJsonRecord(value)) {
    return Object.fromEntries(
      Object.entries(value)
        .map(([k, v]) => [k, dropUnsupportedJsonValues(v)])
        .filter(([, v]) => v !== undefined),
    )
  }

  return value
}

function isJsonRecord(value: unknown): value is JsonRecord {
  return typeof value === 'object' && value !== null
}

function normalizeLegacyOptionalUint64(obj: JsonRecord, key: string): void {
  const value = obj[key]
  if (typeof value !== 'string') return

  const trimmed = value.trim()
  if (!isPositiveUint64String(trimmed)) {
    delete obj[key]
    return
  }

  obj[key] = trimmed
}

function isPositiveUint64String(value: string): boolean {
  if (!/^\d+$/.test(value)) return false

  const n = BigInt(value)
  return n > 0n && n <= UINT64_MAX
}
