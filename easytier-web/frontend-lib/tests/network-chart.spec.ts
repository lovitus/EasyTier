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
  let observedResize: ResizeObserverCallback | undefined
  let canvasWidth = 480
  let canvasHeight = 128

  beforeEach(() => {
    chartState.instances.length = 0
    observedResize = undefined
    canvasWidth = 480
    canvasHeight = 128

    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({} as CanvasRenderingContext2D)
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function () {
      return this instanceof HTMLCanvasElement ? canvasWidth : 480
    })
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function () {
      return this instanceof HTMLCanvasElement ? canvasHeight : 128
    })

    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: ResizeObserverCallback) {
        observedResize = callback
      }

      observe() {}
      disconnect() {}
    })
  })

  it('adds one sample for each polling tick', async () => {
    const wrapper = mount(NetworkChart, {
      props: {
        uploadRate: '1.0 KiB',
        downloadRate: '2.0 KiB',
        tick: 1,
      },
    })
    await flushPromises()

    expect(chartState.instances).toHaveLength(1)
    const chart = chartState.instances[0]
    expect(chart.update).toHaveBeenCalledTimes(1)

    await wrapper.setProps({
      uploadRate: '3.0 KiB',
      downloadRate: '4.0 KiB',
      tick: 2,
    })
    await nextTick()

    expect(chart.update).toHaveBeenCalledTimes(2)
    expect(chart.data.datasets[0].data.at(-1)).toBe(3 * 1024)
    expect(chart.data.datasets[1].data.at(-1)).toBe(4 * 1024)
    wrapper.unmount()
  })

  it('initializes after a hidden canvas becomes visible', async () => {
    canvasWidth = 0
    canvasHeight = 0
    const wrapper = mount(NetworkChart, {
      props: {
        uploadRate: '0',
        downloadRate: '0',
        tick: 1,
      },
    })
    await flushPromises()

    expect(chartState.instances).toHaveLength(0)
    expect(observedResize).toBeDefined()

    canvasWidth = 480
    canvasHeight = 128
    observedResize!([] as unknown as ResizeObserverEntry[], {} as ResizeObserver)

    expect(chartState.instances).toHaveLength(1)
    expect(chartState.instances[0].resize).toHaveBeenCalledOnce()
    wrapper.unmount()
  })
})
