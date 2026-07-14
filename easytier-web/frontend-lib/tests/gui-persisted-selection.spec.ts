import { describe, expect, it } from 'vitest'
import {
  loadInitialNetworkInstanceId,
  loadLastNetworkInstanceId,
  saveLastNetworkInstanceId,
  type ConfigStorage,
} from '../../../easytier-gui/src/composables/config'

function memoryStorage(initialValue: string | null = null): ConfigStorage {
  const values = new Map<string, string>()
  if (initialValue !== null) values.set('last_network_instance_id', initialValue)
  return {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  }
}

describe('GUI persisted network selection', () => {
  it('restores a persisted instance before the management component mounts', () => {
    const storage = memoryStorage('c17a8c16-5016-4d09-a1c3-e97c6fddcaf5')

    expect(loadInitialNetworkInstanceId(storage)).toBe('c17a8c16-5016-4d09-a1c3-e97c6fddcaf5')
  })

  it('normalizes empty persisted values to no selection', () => {
    expect(loadLastNetworkInstanceId(memoryStorage('  '))).toBeNull()
    expect(loadInitialNetworkInstanceId(memoryStorage())).toBeUndefined()
  })

  it('preserves the storage key used by existing installations', () => {
    const storage = memoryStorage()
    saveLastNetworkInstanceId('saved-instance', storage)

    expect(loadLastNetworkInstanceId(storage)).toBe('saved-instance')
  })
})
