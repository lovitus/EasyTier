import { UUID } from './utils';
import { NetworkConfig, NetworkInstanceRunningInfo } from '../types/network';

export interface ValidateConfigResponse {
    toml_config: string;
    policy_diagnostics?: Array<{
        severity: string;
        code: string;
        path: string;
        message: string;
    }>;
}

export type PolicyRuleDataResource = 'geosite' | 'geoip' | 'mmdb';

export interface UpdatePolicyRuleDataResponse {
    path: string;
    sha256: string;
    size: number | string;
    source_url: string;
}

export interface PolicyOutboundInterface {
    name: string;
    addresses: string[];
    recommended: boolean;
}

export interface ListPolicyOutboundInterfacesResponse {
    platform: string;
    required: boolean;
    supported: boolean;
    interfaces: PolicyOutboundInterface[];
}

export interface ListNetworkInstanceIdResponse {
    running_inst_ids: Array<UUID>,
    disabled_inst_ids: Array<UUID>,
}

export interface GenerateConfigResponse {
    toml_config?: string;
    error?: string;
}

export interface ParseConfigResponse {
    config?: NetworkConfig;
    error?: string;
}

export interface CollectNetworkInfoResponse {
    info: {
        map: Record<string, NetworkInstanceRunningInfo | undefined>;
    }
}

export namespace ConfigFilePermission {
    export type Flags = number;
    export const READ_ONLY: Flags = 1 << 0;
    export const NO_DELETE: Flags = 1 << 1;
    export function hasPermission(perm: Flags, flag: Flags): boolean {
        return (perm & flag) === flag;
    }
    export function isRemoveSaveable(perm: Flags): boolean {
        return !hasPermission(perm, NO_DELETE);
    }
    export function isEditable(perm: Flags): boolean {
        return !hasPermission(perm, READ_ONLY);
    }
    export function isDeletable(perm: Flags): boolean {
        return !hasPermission(perm, NO_DELETE);
    }
}

export interface NetworkMeta {
    network_name: string;
    config_permission: ConfigFilePermission.Flags;
}

export interface GetNetworkMetasResponse {
    metas: Record<string, NetworkMeta>;
}

export interface RemoteClient {
    validate_config(config: NetworkConfig): Promise<ValidateConfigResponse>;
    update_policy_rule_data?(inst_id: string, resource: PolicyRuleDataResource, source_url?: string): Promise<UpdatePolicyRuleDataResponse>;
    list_policy_outbound_interfaces?(): Promise<ListPolicyOutboundInterfacesResponse>;
    run_network(config: NetworkConfig, save: boolean): Promise<undefined>;
    get_network_info(inst_id: string): Promise<NetworkInstanceRunningInfo | undefined>;
    list_network_instance_ids(): Promise<ListNetworkInstanceIdResponse>;
    delete_network(inst_id: string): Promise<undefined>;
    update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined>;
    save_config(config: NetworkConfig): Promise<undefined>;
    get_network_config(inst_id: string): Promise<NetworkConfig>;
    generate_config(config: NetworkConfig): Promise<GenerateConfigResponse>;
    parse_config(toml_config: string): Promise<ParseConfigResponse>;
    get_network_metas(instance_ids: string[]): Promise<GetNetworkMetasResponse>;
}
