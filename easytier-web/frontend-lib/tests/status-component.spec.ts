import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import Status from '../src/components/Status.vue'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

vi.mock('../src/components/NetworkChart.vue', () => ({
  default: defineComponent({
    name: 'NetworkChart',
    props: {
      uploadRate: String,
      downloadRate: String,
    },
    setup(props) {
      return () => h('div', {
        'data-stub': 'network-chart',
        'data-upload': props.uploadRate,
        'data-download': props.downloadRate,
      })
    },
  }),
}))

vi.mock('primevue', async () => {
  const { defineComponent, h } = await import('vue')

  const PassThrough = defineComponent({
    name: 'PassThrough',
    setup(_, { slots }) {
      return () => h('div', slots.default?.())
    },
  })

  const CardStub = defineComponent({
    name: 'Card',
    setup(_, { slots }) {
      return () => h('section', { class: 'p-card' }, [
        h('header', { class: 'p-card-title' }, slots.title?.()),
        h('div', { class: 'p-card-content' }, slots.content?.()),
      ])
    },
  })

  const ColumnStub = defineComponent({
    name: 'Column',
    props: {
      field: [String, Function],
      header: String,
    },
    setup() {
      return () => null
    },
  })

  const DataTableStub = defineComponent({
    name: 'DataTable',
    props: {
      value: {
        type: Array,
        default: () => [],
      },
    },
    setup(props, { slots }) {
      return () => {
        const columns = (slots.default?.() ?? []).filter(Boolean)
        return h('table', { class: 'p-datatable' }, [
          h('tbody', { class: 'p-datatable-tbody' }, (props.value as any[]).map((row, rowIndex) => h('tr', { key: row.ui_key ?? rowIndex }, columns.map((column, columnIndex) => {
            const bodySlot = (column.children as any)?.body
            const field = column.props?.field
            const value = typeof field === 'function' ? field(row) : field ? row[field] : ''
            const content = bodySlot ? bodySlot({ data: row, index: rowIndex }) : value
            return h('td', { key: columnIndex }, content)
          }))))
        ])
      }
    },
  })

  return {
    Badge: PassThrough,
    Button: PassThrough,
    Card: CardStub,
    Chip: PassThrough,
    Column: ColumnStub,
    DataTable: DataTableStub,
    Dialog: PassThrough,
    Divider: PassThrough,
    ScrollPanel: PassThrough,
    Tag: PassThrough,
    Timeline: PassThrough,
  }
})

describe('Status mixed-version rendering', () => {
  it('renders peers without route feature_flag', () => {
    const wrapper = mount(Status, {
      props: {
        curNetworkInst: {
          instance_id: 'inst-1',
          running: true,
          error_msg: '',
          detail: {
            dev_name: 'utun9',
            events: [],
            my_node_info: {
              virtual_ipv4: { address: { addr: 0 }, network_length: 24 },
              hostname: 'local',
              version: '2.6.7',
              peer_id: 1,
              listeners: [],
              ips: {
                public_ipv4: { addr: 0 },
                interface_ipv4s: [],
                public_ipv6: { part1: 0, part2: 0, part3: 0, part4: 0 },
                interface_ipv6s: [],
                listeners: [],
              },
              stun_info: {
                udp_nat_type: 0,
                tcp_nat_type: 0,
                last_update_time: 0,
              },
            },
            peer_route_pairs: [
              {
                route: {
                  cost: 1,
                  hostname: 'legacy-peer',
                  ipv4_addr: { address: { addr: 0 }, network_length: 24 },
                  next_hop_peer_id: 2,
                  peer_id: 2,
                  proxy_cidrs: [],
                  inst_id: 'inst-1',
                  version: '2.6.5',
                },
                peer: {
                  peer_id: 2,
                  conns: [],
                },
              },
            ],
            peers: [],
            routes: [],
            running: true,
          },
        },
      },
      global: {
        stubs: {
          HumanEvent: true,
        },
        directives: {
          tooltip: () => {},
        },
      },
    })

    expect(wrapper.text()).toContain('legacy-peer')
    expect(wrapper.find('[data-stub="network-chart"]').exists()).toBe(true)
  })

  it('renders chart and proxy failover entries from camelCase detail fields', () => {
    const wrapper = mount(Status, {
      props: {
        curNetworkInst: {
          instance_id: 'inst-2',
          running: true,
          error_msg: '',
          detail: {
            devName: 'utun10',
            events: [],
            myNodeInfo: {
              virtual_ipv4: { address: { addr: 0 }, network_length: 24 },
              hostname: 'local-camel',
              version: '2.6.7',
              peer_id: 7,
              listeners: [],
              ips: {
                public_ipv4: { addr: 0 },
                interface_ipv4s: [],
                public_ipv6: { part1: 0, part2: 0, part3: 0, part4: 0 },
                interface_ipv6s: [],
                listeners: [],
              },
              stun_info: {
                udp_nat_type: 0,
                tcp_nat_type: 0,
                last_update_time: 0,
              },
            },
            peerRoutePairs: [
              {
                route: {
                  cost: 1,
                  hostname: 'camel-peer',
                  ipv4_addr: { address: { addr: 0 }, network_length: 24 },
                  next_hop_peer_id: 8,
                  peer_id: 8,
                  proxy_cidrs: [],
                  inst_id: 'inst-2',
                  version: '2.6.7',
                },
                peer: {
                  peer_id: 8,
                  conns: [],
                },
              },
            ],
            proxyFailoverEntries: [
              {
                src: {
                  ip: { oneofKind: 'ipv4', ipv4: { addr: 0 } },
                  port: 1000,
                },
                dst: {
                  ip: { oneofKind: 'ipv4', ipv4: { addr: 0 } },
                  port: 2000,
                },
                requestedTransport: 'quic,kcp,native',
                selectedTransport: 'native',
                fallbackReason: 'quic_policy_denied,kcp_policy_denied',
                dstPeerId: 1981135380,
                generation: 3,
              },
            ],
            peers: [],
            routes: [],
            running: true,
          },
        },
      },
      global: {
        stubs: {
          HumanEvent: true,
        },
        directives: {
          tooltip: () => {},
        },
      },
    })

    expect(wrapper.find('[data-stub="network-chart"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('camel-peer')
    expect(wrapper.text()).toContain('quic,kcp,native')
    expect(wrapper.text()).toContain('quic_policy_denied,kcp_policy_denied')
  })
})
