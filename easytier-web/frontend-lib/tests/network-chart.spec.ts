import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import NetworkChart from '../src/components/NetworkChart.vue'

const chartState = vi.hoisted(() => ({
  instances: [] as any[],
}))

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

vi.mock('chart.js', () => {
  class ChartMock {
    static register = vi.fn()
    data: any
    update = vi.fn()
    resize = vi.fn()
    destroy = vi.fn()

    constructor(_context: unknown, config: any) {
      this.data = config.data
      chartState.instances.push(this)
    }
  }

  return {
    Chart: ChartMock,
    CategoryScale: {},
    LinearScale: {},
    PointElement: {},
    LineElement: {},
    LineController: {},
    Title: {},
    Tooltip: {},
    Legend: {},
    Filler: {},
  }
})

describe('NetworkChart lifecycle', () => {
  beforeEach(() => {
    chartState.instances.length = 0

    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({} as CanvasRenderingContext2D)
  })

  it('initializes once and adds one sample when rates change', async () => {
    const wrapper = mount(NetworkChart, {
      props: {
        uploadRate: '1.0 KiB',
        downloadRate: '2.0 KiB',
      },
    })
    await flushPromises()

    expect(chartState.instances).toHaveLength(1)
    expect(wrapper.find('.network-chart-canvas').exists()).toBe(true)
    expect(wrapper.find('.network-chart-canvas canvas').exists()).toBe(true)
    const chart = chartState.instances[0]
    expect(chart.update).toHaveBeenCalledTimes(1)

    await wrapper.setProps({
      uploadRate: '3.0 KiB',
      downloadRate: '4.0 KiB',
    })
    await nextTick()

    expect(chart.update).toHaveBeenCalledTimes(2)
    expect(chart.data.datasets[0].data.at(-1)).toBe(3 * 1024)
    expect(chart.data.datasets[1].data.at(-1)).toBe(4 * 1024)
    wrapper.unmount()
  })

  it('uses numeric rate props before string fallback', async () => {
    const wrapper = mount(NetworkChart, {
      props: {
        uploadRate: '1.0 KiB',
        downloadRate: '2.0 KiB',
        uploadRateBytes: 512,
        downloadRateBytes: 1024,
      },
    })
    await flushPromises()

    const chart = chartState.instances[0]
    expect(chart.data.datasets[0].data.at(-1)).toBe(512)
    expect(chart.data.datasets[1].data.at(-1)).toBe(1024)

    await wrapper.setProps({
      uploadRateBytes: undefined,
      downloadRateBytes: undefined,
      uploadRate: '3.0 KiB',
      downloadRate: '4.0 KiB',
    })
    await nextTick()

    expect(chart.data.datasets[0].data.at(-1)).toBe(3 * 1024)
    expect(chart.data.datasets[1].data.at(-1)).toBe(4 * 1024)
    wrapper.unmount()
  })
})
