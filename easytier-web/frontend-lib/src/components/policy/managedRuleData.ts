import type * as Api from '../../modules/api'
import type {
  PolicyEditorDocument,
  PolicyRuleSetKind,
  PolicyRuleSetRow,
} from './policyDocument'

export const MANAGED_RULE_DATA = {
  geosite: {
    builtin: true,
    name: 'geosite',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geosite.dat',
  },
  geoip: {
    builtin: true,
    name: 'geoip',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip-lite.dat',
  },
  mmdb: {
    builtin: false,
    name: 'country',
    source: 'https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country-lite.mmdb',
  },
} satisfies Record<PolicyRuleSetKind, { builtin: boolean, name: string, source: string }>

export const MANAGED_RULE_DATA_TYPES = Object.keys(MANAGED_RULE_DATA) as PolicyRuleSetKind[]

function emptyManagedRuleDataRow(type: PolicyRuleSetKind): PolicyRuleSetRow {
  return {
    name: MANAGED_RULE_DATA[type].name,
    type,
    path: '',
    update: 'manual',
    sha256: '',
    sourceUrl: '',
  }
}

export function managedRuleDataRows(document: PolicyEditorDocument): PolicyRuleSetRow[] {
  return MANAGED_RULE_DATA_TYPES.map(type =>
    document.ruleSets.find(row => row.type === type) ?? emptyManagedRuleDataRow(type))
}

export function ensureManagedRuleDataRows(document: PolicyEditorDocument): void {
  for (const type of MANAGED_RULE_DATA_TYPES) {
    if (document.ruleSets.some(row => row.type === type)) continue
    document.ruleSets.push(emptyManagedRuleDataRow(type))
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
  const customSource = row.sourceUrl.trim()
  const result = await api.update_policy_rule_data(
    instanceId,
    row.type,
    customSource || undefined,
  )
  if (result.updated === false) {
    if (!row.path.trim() && result.path.trim() && result.sha256.trim()) {
      row.path = result.path
      row.sha256 = result.sha256
      row.update = 'manual'
      row.sourceUrl = customSource ? result.source_url : ''
    }
    return result
  }
  row.path = result.path
  row.sha256 = result.sha256
  row.update = 'manual'
  row.sourceUrl = customSource ? result.source_url : ''
  return result
}
