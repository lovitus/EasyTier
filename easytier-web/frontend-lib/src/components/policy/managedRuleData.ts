import type * as Api from '../../modules/api'
import type {
  PolicyEditorDocument,
  PolicyRuleSetKind,
  PolicyRuleSetRow,
} from './policyDocument'

export const MANAGED_RULE_DATA = {
  geosite: {
    name: 'geosite',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geosite.dat',
  },
  geoip: {
    name: 'geoip',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip-lite.dat',
  },
  mmdb: {
    name: 'country',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country-lite.mmdb',
  },
} satisfies Record<PolicyRuleSetKind, { name: string, source: string }>

export const MANAGED_RULE_DATA_TYPES = Object.keys(MANAGED_RULE_DATA) as PolicyRuleSetKind[]

export function ensureManagedRuleDataRows(document: PolicyEditorDocument): void {
  for (const type of MANAGED_RULE_DATA_TYPES) {
    if (document.ruleSets.some(row => row.type === type)) continue
    document.ruleSets.push({
      name: MANAGED_RULE_DATA[type].name,
      type,
      path: '',
      update: 'manual',
      sha256: '',
    })
  }
}

export async function updateManagedRuleData(
  api: Api.RemoteClient,
  instanceId: string,
  row: PolicyRuleSetRow,
): Promise<Api.UpdatePolicyRuleDataResponse> {
  if (!api.update_policy_rule_data) {
    throw new Error('policy rule data update is not supported by this client')
  }
  const result = await api.update_policy_rule_data(instanceId, row.type)
  row.path = result.path
  row.sha256 = result.sha256
  row.update = 'manual'
  return result
}
