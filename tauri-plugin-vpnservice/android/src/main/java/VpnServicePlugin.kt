package com.plugin.vpnservice

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import androidx.activity.result.ActivityResult
import app.tauri.annotation.Command
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import android.webkit.WebView
import org.json.JSONArray

@InvokeArg
class PingArgs {
    var value: String? = null
}

@InvokeArg
class StartVpnArgs {
    var instanceId: String? = null
    var ipv4Addr: String? = null
    var routes: Array<String> = emptyArray()
    var dns: String? = null
    var disallowedApplications: Array<String> = emptyArray()
    var mtu: Int? = null
}

@TauriPlugin
class VpnServicePlugin(private val activity: Activity) : Plugin(activity) {
    private val implementation = Example()

    override fun load(webView: WebView) {
        println("load vpn service plugin")
        TauriVpnService.triggerCallback = { event, data ->
            println("vpn: triggerCallback $event $data")
            trigger(event, data)
        }
    }

    @Command
    fun ping(invoke: Invoke) {
        val args = invoke.parseArgs(PingArgs::class.java)

        val ret = JSObject()
        ret.put("value", implementation.pong(args.value ?: "default value :("))
        invoke.resolve(ret)
    }

    @Command
    fun prepareVpn(invoke: Invoke) {
        activity.runOnUiThread {
            println("prepare vpn in plugin")
            // After the first installation grant Android returns null here, so ordinary
            // stop/start cycles perform only this silent ownership check and show no dialog.
            val it = VpnService.prepare(activity)
            if (it != null) {
                startActivityForResult(invoke, it, "onPrepareVpnResult")
                return@runOnUiThread
            }
            TauriVpnService.setRevokedBySystem(activity, false)
            val ret = JSObject()
            ret.put("granted", true)
            invoke.resolve(ret)
        }
    }

    @ActivityCallback
    fun onPrepareVpnResult(invoke: Invoke, result: ActivityResult) {
        val granted = result.resultCode == Activity.RESULT_OK
        TauriVpnService.setRevokedBySystem(activity, !granted)
        val ret = JSObject()
        ret.put("granted", granted)
        invoke.resolve(ret)
    }

    @Command
    fun startVpn(invoke: Invoke) {
        val args = invoke.parseArgs(StartVpnArgs::class.java)
        activity.runOnUiThread {
            println("start vpn in plugin, args: $args")

            val ret = JSObject()
            if (TauriVpnService.wasRevokedBySystem(activity)) {
                ret.put("errorMsg", "vpn_revoked")
            } else if (VpnService.prepare(activity) != null) {
                TauriVpnService.setRevokedBySystem(activity, true)
                ret.put("errorMsg", "need_prepare")
            } else {
                TauriVpnService.self?.prepareForRestart()
                val intent = Intent(activity, TauriVpnService::class.java)
                intent.putExtra(TauriVpnService.INSTANCE_ID, args.instanceId)
                intent.putExtra(TauriVpnService.IPV4_ADDR, args.ipv4Addr)
                intent.putExtra(TauriVpnService.ROUTES, args.routes)
                intent.putExtra(TauriVpnService.DNS, args.dns)
                intent.putExtra(TauriVpnService.DISALLOWED_APPLICATIONS, args.disallowedApplications)
                intent.putExtra(TauriVpnService.MTU, args.mtu)

                activity.startService(intent)
            }
            invoke.resolve(ret)
        }
    }

    @Command
    fun stopVpn(invoke: Invoke) {
        activity.runOnUiThread {
            println("stop vpn in plugin")
            TauriVpnService.self?.stopByUser()
            activity.stopService(Intent(activity, TauriVpnService::class.java))
            println("stop vpn in plugin end")
            invoke.resolve(JSObject())
        }
    }

    @Command
    fun getVpnStatus(invoke: Invoke) {
        val ret = JSObject()
        ret.put("running", TauriVpnService.self?.isRunning() == true)
        ret.put("ipv4Addr", TauriVpnService.ipv4Addr)
        ret.put("routes", JSONArray(TauriVpnService.routes.toList()))
        ret.put("dns", TauriVpnService.dns)
        ret.put("revokedBySystem", TauriVpnService.wasRevokedBySystem(activity))
        invoke.resolve(ret)
    }
}
