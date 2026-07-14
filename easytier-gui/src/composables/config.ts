/**
 * 配置持久化相关的函数
 * 用于保存和加载应用程序的各种配置状态
 */

const LAST_NETWORK_INSTANCE_ID_KEY = 'last_network_instance_id'

export interface ConfigStorage {
    getItem(key: string): string | null
    setItem(key: string, value: string): void
}

function normalizeStoredInstanceId(instanceId: string | null): string | null {
    const normalized = instanceId?.trim()
    return normalized ? normalized : null
}

/**
 * 保存上次使用的网络实例 ID
 * @param instanceId 网络实例 ID
 */
export function saveLastNetworkInstanceId(
    instanceId: string,
    storage: Pick<ConfigStorage, 'setItem'> = localStorage,
) {
    storage.setItem(LAST_NETWORK_INSTANCE_ID_KEY, instanceId)
}

/**
 * 加载上次使用的网络实例 ID
 * @returns 上次使用的网络实例 ID，如果没有则返回 null
 */
export function loadLastNetworkInstanceId(
    storage: Pick<ConfigStorage, 'getItem'> = localStorage,
): string | null {
    return normalizeStoredInstanceId(storage.getItem(LAST_NETWORK_INSTANCE_ID_KEY))
}

/**
 * Resolve the initial selection before RemoteManagement mounts.
 * This avoids relying on a later client-running state transition that may not occur.
 */
export function loadInitialNetworkInstanceId(
    storage: Pick<ConfigStorage, 'getItem'> = localStorage,
): string | undefined {
    return loadLastNetworkInstanceId(storage) ?? undefined
}
