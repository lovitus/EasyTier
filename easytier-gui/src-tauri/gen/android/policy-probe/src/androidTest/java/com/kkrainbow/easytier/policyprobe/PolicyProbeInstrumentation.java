package com.kkrainbow.easytier.policyprobe;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import android.os.Process;
import android.os.SystemClock;

import java.io.BufferedReader;
import java.io.FileReader;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.util.Collections;

import javax.net.ssl.SNIHostName;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;

/**
 * Runs one bounded TCP connection from the target application's UID and process domain.
 * A rejected connection is an observation, not an instrumentation execution failure.
 */
public final class PolicyProbeInstrumentation extends Instrumentation {
    private static final int DEFAULT_TIMEOUT_MS = 3000;
    private static final int MAX_TIMEOUT_MS = 10000;

    private Bundle arguments = new Bundle();

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        if (arguments != null) {
            this.arguments = arguments;
        }
        start();
    }

    @Override
    public void onStart() {
        Bundle result = new Bundle();
        String host = normalizedHost(arguments.getString("host"));
        String tlsServerName = normalizedHost(arguments.getString("tls_server_name"));
        int port = boundedInteger(arguments.getString("port"), 0, 1, 65535);
        int timeoutMs = boundedInteger(
                arguments.getString("timeout_ms"),
                DEFAULT_TIMEOUT_MS,
                100,
                MAX_TIMEOUT_MS
        );

        result.putString("probe_uid", Integer.toString(Process.myUid()));
        result.putString("probe_selinux_context", readSelinuxContext());
        result.putString("probe_target", host + ":" + port);
        result.putString("probe_protocol", tlsServerName.isEmpty() ? "tcp" : "tls");
        result.putString("probe_tls_server_name", tlsServerName);
        result.putString("probe_timeout_ms", Integer.toString(timeoutMs));

        if (host.isEmpty() || port == 0) {
            result.putString("probe_valid", "false");
            result.putString("probe_connected", "false");
            result.putString("probe_tcp_connected", "false");
            result.putString("probe_tls_handshake", "false");
            result.putString("probe_error", "host and port are required");
            finish(Activity.RESULT_CANCELED, result);
            return;
        }

        result.putString("probe_valid", "true");
        long startedAt = SystemClock.elapsedRealtime();
        boolean connected = false;
        boolean tcpConnected = false;
        boolean tlsHandshake = false;
        String error = "";
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(host, port), timeoutMs);
            tcpConnected = true;
            if (tlsServerName.isEmpty()) {
                connected = true;
            } else {
                socket.setSoTimeout(timeoutMs);
                SSLSocketFactory factory = (SSLSocketFactory) SSLSocketFactory.getDefault();
                try (SSLSocket tlsSocket = (SSLSocket) factory.createSocket(
                        socket,
                        tlsServerName,
                        port,
                        true
                )) {
                    SSLParameters parameters = tlsSocket.getSSLParameters();
                    parameters.setEndpointIdentificationAlgorithm("HTTPS");
                    parameters.setServerNames(Collections.singletonList(new SNIHostName(tlsServerName)));
                    tlsSocket.setSSLParameters(parameters);
                    tlsSocket.setSoTimeout(timeoutMs);
                    tlsSocket.startHandshake();
                    tlsHandshake = true;
                    connected = true;
                }
            }
        } catch (Exception exception) {
            String message = exception.getMessage();
            error = exception.getClass().getSimpleName()
                    + (message == null || message.isEmpty() ? "" : ": " + message);
        }

        result.putString("probe_connected", Boolean.toString(connected));
        result.putString("probe_tcp_connected", Boolean.toString(tcpConnected));
        result.putString("probe_tls_handshake", Boolean.toString(tlsHandshake));
        result.putString(
                "probe_elapsed_ms",
                Long.toString(SystemClock.elapsedRealtime() - startedAt)
        );
        result.putString("probe_error", error);
        finish(Activity.RESULT_OK, result);
    }

    private static String normalizedHost(String host) {
        return host == null ? "" : host.trim();
    }

    private static int boundedInteger(String raw, int fallback, int minimum, int maximum) {
        if (raw == null || raw.trim().isEmpty()) {
            return fallback;
        }
        try {
            int value = Integer.parseInt(raw.trim());
            return value >= minimum && value <= maximum ? value : fallback;
        } catch (NumberFormatException ignored) {
            return fallback;
        }
    }

    private static String readSelinuxContext() {
        try (BufferedReader reader = new BufferedReader(new FileReader("/proc/self/attr/current"))) {
            String value = reader.readLine();
            return value == null ? "unknown" : value.trim();
        } catch (Exception exception) {
            return "unavailable:" + exception.getClass().getSimpleName();
        }
    }
}
