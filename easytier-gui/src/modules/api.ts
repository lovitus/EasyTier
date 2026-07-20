import { type Api, type NetworkTypes } from "easytier-frontend-lib";
import { type } from "@tauri-apps/plugin-os";
import * as backend from "~/composables/backend";
import { prepareVpnService } from "~/composables/mobile_vpn";

export class GUIRemoteClient implements Api.RemoteClient {
    async validate_config(config: NetworkTypes.NetworkConfig): Promise<Api.ValidateConfigResponse> {
        return backend.validateConfig(config);
    }
    async update_policy_rule_data(instanceId: string, resource: Api.PolicyRuleDataResource, sourceUrl?: string): Promise<Api.UpdatePolicyRuleDataResponse> {
        return backend.updatePolicyRuleData(instanceId, resource, sourceUrl);
    }
    async list_policy_rule_data_categories(instanceId: string, resource: Api.PolicyRuleDataResource, expectedSha256?: string, path?: string): Promise<Api.ListPolicyRuleDataCategoriesResponse> {
        return backend.listPolicyRuleDataCategories(instanceId, resource, expectedSha256, path);
    }
    async list_policy_outbound_interfaces(): Promise<Api.ListPolicyOutboundInterfacesResponse> {
        return backend.listPolicyOutboundInterfaces();
    }
    async run_network(config: NetworkTypes.NetworkConfig, save: boolean): Promise<undefined> {
        if (type() === 'android') {
            await prepareVpnService(config.no_tun ?? false);
        }
        await backend.runNetworkInstance(config, save);
    }
    async get_network_info(inst_id: string): Promise<NetworkTypes.NetworkInstanceRunningInfo | undefined> {
        return backend.collectNetworkInfo(inst_id).then(infos => infos?.info?.map?.[inst_id]);
    }
    async list_network_instance_ids(): Promise<Api.ListNetworkInstanceIdResponse> {
        return backend.listNetworkInstanceIds();
    }
    async delete_network(inst_id: string): Promise<undefined> {
        await backend.deleteNetworkInstance(inst_id);
    }
    async update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined> {
        if (type() === 'android' && !disabled) {
            const config = await backend.getConfig(inst_id);
            await prepareVpnService(config.no_tun ?? false);
        }
        await backend.updateNetworkConfigState(inst_id, disabled);
    }
    async save_config(config: NetworkTypes.NetworkConfig): Promise<undefined> {
        await backend.saveNetworkConfig(config);
    }
    async get_network_config(inst_id: string): Promise<NetworkTypes.NetworkConfig> {
        return backend.getConfig(inst_id);
    }
    async generate_config(config: NetworkTypes.NetworkConfig): Promise<Api.GenerateConfigResponse> {
        try {
            return { toml_config: await backend.parseNetworkConfig(config) };
        } catch (e) {
            return { error: e + "" };
        }
    }
    async parse_config(toml_config: string): Promise<Api.ParseConfigResponse> {
        try {
            return { config: await backend.generateNetworkConfig(toml_config) }
        } catch (e) {
            return { error: e + "" };
        }
    }
    async get_network_metas(instance_ids: string[]): Promise<Api.GetNetworkMetasResponse> {
        return await backend.getNetworkMetas(instance_ids);
    }

}
