package com.kkrainbow.easytier.policyprobe;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import android.os.Process;
import android.os.SystemClock;

import java.io.BufferedReader;
import java.io.FileReader;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Collections;
import java.util.Locale;

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
    private static final int MAX_TIMEOUT_MS = 300000;
    private static final int MAX_HTTP_HEADER_BYTES = 32768;

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
        String httpPath = normalizedHttpPath(arguments.getString("http_path"));
        long expectedBytes = boundedLong(
                arguments.getString("expected_bytes"),
                0,
                0,
                268435456
        );
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
        result.putString(
                "probe_protocol",
                !httpPath.isEmpty() ? "http" : (tlsServerName.isEmpty() ? "tcp" : "tls")
        );
        result.putString("probe_tls_server_name", tlsServerName);
        result.putString("probe_http_path", httpPath);
        result.putString("probe_expected_bytes", Long.toString(expectedBytes));
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
        int httpStatus = 0;
        long bytesReceived = 0;
        long bodyElapsedMs = 0;
        String error = "";
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(host, port), timeoutMs);
            tcpConnected = true;
            socket.setSoTimeout(timeoutMs);
            if (!httpPath.isEmpty()) {
                HttpResult http = downloadHttp(socket, host, port, httpPath, expectedBytes);
                httpStatus = http.status;
                bytesReceived = http.bytesReceived;
                bodyElapsedMs = http.bodyElapsedMs;
                connected = httpStatus >= 200
                        && httpStatus < 300
                        && (expectedBytes == 0 || bytesReceived == expectedBytes);
                if (!connected) {
                    error = "HTTP response mismatch: status=" + httpStatus
                            + " bytes=" + bytesReceived
                            + " expected=" + expectedBytes;
                }
            } else if (tlsServerName.isEmpty()) {
                connected = true;
            } else {
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
        result.putString("probe_http_status", Integer.toString(httpStatus));
        result.putString("probe_bytes_received", Long.toString(bytesReceived));
        result.putString("probe_body_elapsed_ms", Long.toString(bodyElapsedMs));
        result.putString(
                "probe_mbps",
                bodyElapsedMs > 0
                        ? String.format(
                                Locale.ROOT,
                                "%.3f",
                                bytesReceived * 8.0 / bodyElapsedMs / 1000.0
                        )
                        : "0"
        );
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

    private static String normalizedHttpPath(String path) {
        if (path == null) {
            return "";
        }
        String normalized = path.trim();
        if (normalized.isEmpty()) {
            return "";
        }
        if (!normalized.startsWith("/") || normalized.contains("\r") || normalized.contains("\n")) {
            return "";
        }
        return normalized;
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

    private static long boundedLong(String raw, long fallback, long minimum, long maximum) {
        if (raw == null || raw.trim().isEmpty()) {
            return fallback;
        }
        try {
            long value = Long.parseLong(raw.trim());
            return value >= minimum && value <= maximum ? value : fallback;
        } catch (NumberFormatException ignored) {
            return fallback;
        }
    }

    private static HttpResult downloadHttp(
            Socket socket,
            String host,
            int port,
            String path,
            long expectedBytes
    ) throws Exception {
        OutputStream output = socket.getOutputStream();
        String request = "GET " + path + " HTTP/1.1\r\n"
                + "Host: " + host + ":" + port + "\r\n"
                + "Accept: application/octet-stream\r\n"
                + "Connection: close\r\n\r\n";
        output.write(request.getBytes(StandardCharsets.US_ASCII));
        output.flush();

        InputStream input = socket.getInputStream();
        byte[] header = new byte[MAX_HTTP_HEADER_BYTES];
        int headerLength = 0;
        while (headerLength < header.length) {
            int value = input.read();
            if (value < 0) {
                throw new IllegalStateException("HTTP response ended before headers");
            }
            header[headerLength++] = (byte) value;
            if (headerLength >= 4
                    && header[headerLength - 4] == '\r'
                    && header[headerLength - 3] == '\n'
                    && header[headerLength - 2] == '\r'
                    && header[headerLength - 1] == '\n') {
                break;
            }
        }
        if (headerLength == header.length) {
            throw new IllegalStateException("HTTP response headers are too large");
        }

        String headerText = new String(header, 0, headerLength, StandardCharsets.ISO_8859_1);
        String[] lines = headerText.split("\r\n");
        String[] statusParts = lines[0].split(" ", 3);
        if (statusParts.length < 2) {
            throw new IllegalStateException("invalid HTTP status line");
        }
        int status = Integer.parseInt(statusParts[1]);
        long contentLength = -1;
        for (String line : lines) {
            int separator = line.indexOf(':');
            if (separator > 0
                    && line.substring(0, separator).trim().equalsIgnoreCase("Content-Length")) {
                contentLength = Long.parseLong(line.substring(separator + 1).trim());
            }
        }
        if (expectedBytes > 0 && contentLength != expectedBytes) {
            throw new IllegalStateException(
                    "unexpected Content-Length " + contentLength + ", expected " + expectedBytes
            );
        }

        byte[] buffer = new byte[65536];
        long bytesReceived = 0;
        long bodyStartedAt = SystemClock.elapsedRealtime();
        while (contentLength < 0 || bytesReceived < contentLength) {
            int maximum = buffer.length;
            if (contentLength >= 0) {
                maximum = (int) Math.min(maximum, contentLength - bytesReceived);
            }
            int read = input.read(buffer, 0, maximum);
            if (read < 0) {
                break;
            }
            bytesReceived += read;
        }
        return new HttpResult(
                status,
                bytesReceived,
                SystemClock.elapsedRealtime() - bodyStartedAt
        );
    }

    private static final class HttpResult {
        final int status;
        final long bytesReceived;
        final long bodyElapsedMs;

        HttpResult(int status, long bytesReceived, long bodyElapsedMs) {
            this.status = status;
            this.bytesReceived = bytesReceived;
            this.bodyElapsedMs = bodyElapsedMs;
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
