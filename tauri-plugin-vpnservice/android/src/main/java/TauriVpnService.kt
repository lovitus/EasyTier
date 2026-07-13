package com.plugin.vpnservice

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.VpnService
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.Bundle
import org.json.JSONArray
import java.util.concurrent.ConcurrentHashMap

import app.tauri.plugin.JSObject

class TauriVpnService : VpnService() {
    companion object {
        @JvmField var triggerCallback: (String, JSObject) -> Unit = { _, _ -> }
        @JvmField var self: TauriVpnService? = null
        @JvmField var ipv4Addr: String? = null
        @JvmField var routes: Array<String> = emptyArray()
        @JvmField var dns: String? = null

        const val IPV4_ADDR = "IPV4_ADDR"
        const val ROUTES = "ROUTES"
        const val DNS = "DNS"
        const val DISALLOWED_APPLICATIONS = "DISALLOWED_APPLICATIONS"
        const val MTU = "MTU"
        const val INSTANCE_ID = "INSTANCE_ID"
        const val STOP_REASON_REQUESTED = "requested"
        const val STOP_REASON_RESTART = "restart"
        const val STOP_REASON_REVOKED = "revoked"
        const val STOP_REASON_DESTROYED = "destroyed"
        // LinkProperties can change several times during one roam or DHCP renewal.
        const val NETWORK_CHANGE_DEBOUNCE_MS = 2_000L
    }

    private var vpnInterface: ParcelFileDescriptor? = null
    private var instanceId: String? = null
    private var lastUnderlyingDnsServers: Array<String> = emptyArray()
    private var lastUnderlyingNetworkKey: String = ""
    private val mainHandler = Handler(Looper.getMainLooper())
    private val underlyingNetworks = ConcurrentHashMap<Network, UnderlyingNetwork>()
    private var networkCallbackRegistered = false
    @Volatile private var networkOutageObserved = false
    private var networkEpoch = 0L

    private data class UnderlyingNetwork(
        @Volatile var linkProperties: LinkProperties? = null,
        @Volatile var losingUntil: Long = 0,
    )

    private data class SelectedNetwork(
        val network: Network,
        val key: String,
        val dnsServers: Array<String>,
    )

    private fun notifyNetworkState(networkKey: String, dnsServers: Array<String>) {
        if (self != this || vpnInterface == null) return
        val data = JSObject()
        data.put("networkKey", networkKey)
        data.put("dnsServers", JSONArray(dnsServers.toList()))
        triggerCallback("vpn_network_changed", data)
    }

    private val emitNetworkChange = Runnable {
        val selectedNetwork = selectUnderlyingNetwork()
        if (selectedNetwork == null) {
            if (networkOutageObserved) return@Runnable
            networkOutageObserved = true
            networkEpoch += 1
            lastUnderlyingNetworkKey = "outage!$networkEpoch"
            lastUnderlyingDnsServers = emptyArray()
            notifyNetworkState(lastUnderlyingNetworkKey, lastUnderlyingDnsServers)
            return@Runnable
        }
        val recoveredFromOutage = networkOutageObserved
        networkOutageObserved = false
        val selected = selectedNetwork.copy(key = "${selectedNetwork.key}!$networkEpoch")
        if (!recoveredFromOutage
            && selected.key == lastUnderlyingNetworkKey
            && selected.dnsServers.contentEquals(lastUnderlyingDnsServers)) {
            return@Runnable
        }
        lastUnderlyingNetworkKey = selected.key
        lastUnderlyingDnsServers = selected.dnsServers
        if (Build.VERSION.SDK_INT in Build.VERSION_CODES.LOLLIPOP_MR1..Build.VERSION_CODES.P) {
            setUnderlyingNetworks(arrayOf(selected.network))
        }
        notifyNetworkState(selected.key, selected.dnsServers)
    }

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            underlyingNetworks.putIfAbsent(network, UnderlyingNetwork())
            scheduleNetworkChange()
        }

        override fun onLosing(network: Network, maxMsToLive: Int) {
            underlyingNetworks[network]?.losingUntil = System.currentTimeMillis() + maxMsToLive
            scheduleNetworkChange()
        }

        override fun onLost(network: Network) {
            underlyingNetworks.remove(network)
            scheduleNetworkChange()
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            val candidate = UnderlyingNetwork()
            val info = underlyingNetworks[network]
                ?: underlyingNetworks.putIfAbsent(network, candidate)
                ?: candidate
            info.linkProperties = properties
            scheduleNetworkChange()
        }

        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            underlyingNetworks.putIfAbsent(network, UnderlyingNetwork())
            scheduleNetworkChange()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null) {
            // This service has no persisted configuration from which a sticky restart can be
            // reconstructed. Starting with defaults would create a wrong VPN generation.
            stopSelf(startId)
            return START_NOT_STICKY
        }
        // Android may reuse the Service after the plugin manually calls onRevoke().
        self = this
        if (vpnInterface != null) {
            disconnect(STOP_REASON_RESTART)
        }
        println("vpn on start command ${intent?.getExtras()} $intent")
        var args = intent?.getExtras()
        ipv4Addr = args?.getString(IPV4_ADDR)
        routes = args?.getStringArray(ROUTES) ?: emptyArray()
        dns = args?.getString(DNS)
        instanceId = args?.getString(INSTANCE_ID)

        val newVpnInterface = createVpnInterface(args)
        vpnInterface = newVpnInterface
        println("vpn created ${newVpnInterface.fd}")

        var event_data = JSObject()
        event_data.put("fd", newVpnInterface.fd)
        event_data.put("dnsServers", JSONArray(lastUnderlyingDnsServers.toList()))
        event_data.put("networkKey", lastUnderlyingNetworkKey)
        event_data.put("instanceId", instanceId)
        triggerCallback("vpn_service_start", event_data)

        return START_NOT_STICKY
    }

    override fun onCreate() {
        super.onCreate()
        self = this
        registerNetworkObserver()
        println("vpn on create")
    }

    override fun onDestroy() {
        println("vpn on destroy")
        unregisterNetworkObserver()
        disconnect(STOP_REASON_DESTROYED)
        self = null
        super.onDestroy()
    }

    override fun onRevoke() {
        println("vpn on revoke")
        // Preserve the platform revoke reason before VpnService stops this Service. The UI uses
        // it to suppress every automatic restart until the user explicitly starts EasyTier again.
        disconnect(STOP_REASON_REVOKED)
        self = null
        super.onRevoke()
    }

    fun prepareForRestart() {
        // Replacing an interface must not call VpnService.onRevoke(): the platform
        // implementation stops the Service and races the immediately following startService().
        disconnect(STOP_REASON_RESTART)
        self = this
    }

    fun stopByUser() {
        disconnect(STOP_REASON_REQUESTED)
    }

    private fun disconnect(reason: String) {
        val activeInterface = vpnInterface
        vpnInterface = null
        if (self == this && activeInterface != null) {
            val data = JSObject()
            data.put("reason", reason)
            data.put("instanceId", instanceId)
            triggerCallback("vpn_service_stop", data)
        }
        try {
            activeInterface?.close()
        } finally {
            clearStatus()
        }
    }

    fun isRunning(): Boolean = self == this && vpnInterface != null

    private fun clearStatus() {
        ipv4Addr = null
        routes = emptyArray()
        dns = null
        lastUnderlyingDnsServers = emptyArray()
        lastUnderlyingNetworkKey = ""
        networkOutageObserved = false
        networkEpoch = 0
        instanceId = null
    }

    private fun registerNetworkObserver() {
        if (networkCallbackRegistered) return
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        val connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        try {
            connectivityManager.registerNetworkCallback(request, networkCallback)
            networkCallbackRegistered = true
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                connectivityManager.activeNetwork?.let { network ->
                    val capabilities = connectivityManager.getNetworkCapabilities(network)
                    if (isUsableUnderlying(capabilities)) {
                        underlyingNetworks[network] = UnderlyingNetwork(
                            connectivityManager.getLinkProperties(network)
                        )
                    }
                }
            }
        } catch (error: Exception) {
            println("vpn network observer registration failed: $error")
        }
    }

    private fun unregisterNetworkObserver() {
        mainHandler.removeCallbacks(emitNetworkChange)
        if (networkCallbackRegistered) {
            val connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            try {
                connectivityManager.unregisterNetworkCallback(networkCallback)
            } catch (error: Exception) {
                println("vpn network observer unregister failed: $error")
            }
        }
        networkCallbackRegistered = false
        underlyingNetworks.clear()
    }

    private fun scheduleNetworkChange() {
        mainHandler.removeCallbacks(emitNetworkChange)
        mainHandler.postDelayed(emitNetworkChange, NETWORK_CHANGE_DEBOUNCE_MS)
    }

    private fun isUsableUnderlying(capabilities: NetworkCapabilities?): Boolean {
        return capabilities != null
            && capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            && capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private fun networkPriority(capabilities: NetworkCapabilities?, losing: Boolean): Int {
        val base = when {
            capabilities == null -> 100
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> 0
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> 1
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
                && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_USB) -> 2
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH) -> 3
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> 4
            else -> 20
        }
        return base + if (losing) 10 else 0
    }

    private fun selectUnderlyingNetwork(): SelectedNetwork? {
        val connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            val active = connectivityManager.activeNetwork
            if (active != null
                && isUsableUnderlying(connectivityManager.getNetworkCapabilities(active))) {
                underlyingNetworks.putIfAbsent(
                    active,
                    UnderlyingNetwork(connectivityManager.getLinkProperties(active)),
                )
            }
        }
        val now = System.currentTimeMillis()
        val selected = underlyingNetworks.entries
            .filter { isUsableUnderlying(connectivityManager.getNetworkCapabilities(it.key)) }
            .minByOrNull {
                networkPriority(
                    connectivityManager.getNetworkCapabilities(it.key),
                    it.value.losingUntil > now,
                )
            }
            ?: return null
        val properties = selected.value.linkProperties
            ?: connectivityManager.getLinkProperties(selected.key)
            ?: return null
        val dnsServers = properties.dnsServers
            .mapNotNull { it.hostAddress }
            .filter { !it.contains('%') }
            .distinct()
            .take(4)
            .toTypedArray()
        if (dnsServers.isEmpty()) return null
        val capabilities = connectivityManager.getNetworkCapabilities(selected.key)
        val transport = when {
            capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true -> "wifi"
            capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true -> "ethernet"
            capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) == true -> "cellular"
            else -> "other"
        }
        val linkSignature = listOf(
            properties.interfaceName.orEmpty(),
            properties.linkAddresses.map { it.toString() }.sorted().joinToString(","),
            properties.routes.map { it.toString() }.sorted().joinToString(","),
            dnsServers.joinToString(","),
        ).joinToString("|").let { Integer.toHexString(it.hashCode()) }
        return SelectedNetwork(
            selected.key,
            "${selected.key}@$transport#$linkSignature",
            dnsServers,
        )
    }

    private fun createVpnInterface(args: Bundle?): ParcelFileDescriptor {
        val selectedNetwork = selectUnderlyingNetwork()?.let {
            it.copy(key = "${it.key}!$networkEpoch")
        }
        networkOutageObserved = selectedNetwork == null
        val underlyingDnsServers = selectedNetwork?.dnsServers ?: emptyArray()
        var builder = Builder()
                .setSession("TauriVpnService")
                .setBlocking(false)
        
        var mtu = args?.getInt(MTU) ?: 1500
        var ipv4Addr = args?.getString(IPV4_ADDR) ?: "10.126.126.1/24"
        var dns: String? = args?.getString(DNS)
        var routes = args?.getStringArray(ROUTES) ?: emptyArray()
        var disallowedApplications = args?.getStringArray(DISALLOWED_APPLICATIONS) ?: emptyArray()

        println("vpn create vpn interface. mtu: $mtu, ipv4Addr: $ipv4Addr, dns:" +
            "$dns, routes: ${java.util.Arrays.toString(routes)}," +
            "disallowedApplications:  ${java.util.Arrays.toString(disallowedApplications)}")

        val ipParts = ipv4Addr.split("/")
        if (ipParts.size != 2) throw IllegalArgumentException("Invalid IP addr string")
        builder.addAddress(ipParts[0], ipParts[1].toInt())
        builder.addAddress("fd00::1", 128)

        builder.setMtu(mtu)
        dns?.let { builder.addDnsServer(it) }

        for (route in routes) {
            val ipParts = route.split("/")
            if (ipParts.size != 2) throw IllegalArgumentException("Invalid route cidr string")
            builder.addRoute(ipParts[0], ipParts[1].toInt())
        }
        
        for (app in (disallowedApplications + packageName).distinct()) {
            builder.addDisallowedApplication(app)
        }

        val vpn = builder.also {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                it.setMetered(false)
            }
        }
        .establish()
        ?: throw IllegalStateException("Failed to init VpnService")
        lastUnderlyingDnsServers = underlyingDnsServers
        lastUnderlyingNetworkKey = selectedNetwork?.key.orEmpty()
        if (selectedNetwork != null
            && Build.VERSION.SDK_INT in Build.VERSION_CODES.LOLLIPOP_MR1..Build.VERSION_CODES.P) {
            setUnderlyingNetworks(arrayOf(selectedNetwork.network))
        }
        return vpn
    }
}
