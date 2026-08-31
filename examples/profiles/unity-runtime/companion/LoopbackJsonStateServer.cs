using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;

namespace DccCua.UnityRuntime
{
    internal sealed class LoopbackJsonStateServer : IDisposable
    {
        private const string StatePath = "/v1/ui";
        private const int MaximumHeaderLines = 64;
        private const int MaximumHeaderBytes = 16 * 1024;
        private const int MaximumBodyBytes = 8 * 1024 * 1024;

        private readonly object snapshotLock = new object();
        private readonly int port;
        private TcpListener listener;
        private Thread serverThread;
        private volatile bool stopping;
        private string snapshotJson;
        private string snapshotEtag;

        internal LoopbackJsonStateServer(int port)
        {
            this.port = port;
        }

        internal void Start()
        {
            if (listener != null)
            {
                throw new InvalidOperationException("Unity UI state server has already started");
            }
            stopping = false;
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start(4);
            serverThread = new Thread(ServeLoop)
            {
                IsBackground = true,
                Name = "dcc-cua-unity-ui-state"
            };
            serverThread.Start();
        }

        internal void Publish(string json, string etag)
        {
            if (String.IsNullOrEmpty(json) || Encoding.UTF8.GetByteCount(json) > MaximumBodyBytes)
            {
                throw new InvalidDataException("Unity UI state exceeds the bounded response size");
            }
            lock (snapshotLock)
            {
                snapshotJson = json;
                snapshotEtag = etag;
            }
        }

        public void Dispose()
        {
            stopping = true;
            if (listener != null)
            {
                listener.Stop();
            }
            if (serverThread != null && serverThread.IsAlive)
            {
                serverThread.Join(1000);
            }
            listener = null;
            serverThread = null;
        }

        private void ServeLoop()
        {
            while (!stopping)
            {
                try
                {
                    using (TcpClient client = listener.AcceptTcpClient())
                    {
                        client.ReceiveTimeout = 1000;
                        client.SendTimeout = 1000;
                        HandleClient(client);
                    }
                }
                catch (SocketException)
                {
                    if (!stopping)
                    {
                        Thread.Sleep(50);
                    }
                }
                catch (ObjectDisposedException)
                {
                    return;
                }
                catch (IOException)
                {
                    // A bounded local client may disconnect before a response.
                }
            }
        }

        private void HandleClient(TcpClient client)
        {
            NetworkStream stream = client.GetStream();
            string requestLine;
            string ifNoneMatch = null;
            int bytesRead = 0;
            try
            {
                requestLine = ReadAsciiLine(stream, ref bytesRead);
                if (requestLine == null)
                {
                    return;
                }
                bool headersComplete = false;
                for (int index = 0; index < MaximumHeaderLines; ++index)
                {
                    string line = ReadAsciiLine(stream, ref bytesRead);
                    if (line == null || line.Length == 0)
                    {
                        headersComplete = true;
                        break;
                    }
                    if (line.StartsWith("If-None-Match:", StringComparison.OrdinalIgnoreCase))
                    {
                        ifNoneMatch = line.Substring(line.IndexOf(':') + 1).Trim();
                    }
                }
                if (!headersComplete)
                {
                    WriteStatus(stream, 431, "Request Header Fields Too Large");
                    return;
                }
            }
            catch (InvalidDataException)
            {
                WriteStatus(stream, 431, "Request Header Fields Too Large");
                return;
            }

            string[] request = requestLine.Split(' ');
            if (request.Length != 3 || !String.Equals(request[0], "GET", StringComparison.Ordinal))
            {
                WriteStatus(stream, 405, "Method Not Allowed");
                return;
            }
            if (!String.Equals(request[1], StatePath, StringComparison.Ordinal) ||
                (!String.Equals(request[2], "HTTP/1.1", StringComparison.Ordinal) &&
                 !String.Equals(request[2], "HTTP/1.0", StringComparison.Ordinal)))
            {
                WriteStatus(stream, 404, "Not Found");
                return;
            }

            string body;
            string etag;
            lock (snapshotLock)
            {
                body = snapshotJson;
                etag = snapshotEtag;
            }
            if (String.IsNullOrEmpty(body))
            {
                WriteStatus(stream, 503, "Service Unavailable");
                return;
            }
            if (!String.IsNullOrEmpty(ifNoneMatch) && String.Equals(ifNoneMatch, etag, StringComparison.Ordinal))
            {
                WriteResponse(stream, 304, "Not Modified", etag, null);
                return;
            }
            WriteResponse(stream, 200, "OK", etag, Encoding.UTF8.GetBytes(body));
        }

        private static string ReadAsciiLine(Stream stream, ref int bytesRead)
        {
            List<byte> line = new List<byte>(128);
            while (true)
            {
                int value = stream.ReadByte();
                if (value < 0)
                {
                    return line.Count == 0 ? null : Encoding.ASCII.GetString(line.ToArray());
                }
                ++bytesRead;
                if (bytesRead > MaximumHeaderBytes || value > 127)
                {
                    throw new InvalidDataException("HTTP header is not bounded ASCII");
                }
                if (value == '\n')
                {
                    if (line.Count > 0 && line[line.Count - 1] == '\r')
                    {
                        line.RemoveAt(line.Count - 1);
                    }
                    return Encoding.ASCII.GetString(line.ToArray());
                }
                line.Add((byte)value);
            }
        }

        private static void WriteStatus(Stream stream, int status, string reason)
        {
            WriteResponse(stream, status, reason, null, Encoding.UTF8.GetBytes("{}"));
        }

        private static void WriteResponse(Stream stream, int status, string reason, string etag, byte[] body)
        {
            int length = body == null ? 0 : body.Length;
            StringBuilder headers = new StringBuilder();
            headers.Append("HTTP/1.1 ").Append(status).Append(' ').Append(reason).Append("\r\n");
            headers.Append("Content-Type: application/json\r\n");
            headers.Append("Cache-Control: no-store\r\n");
            headers.Append("Connection: close\r\n");
            if (!String.IsNullOrEmpty(etag))
            {
                headers.Append("ETag: ").Append(etag).Append("\r\n");
            }
            headers.Append("Content-Length: ").Append(length).Append("\r\n\r\n");
            byte[] headerBytes = Encoding.ASCII.GetBytes(headers.ToString());
            stream.Write(headerBytes, 0, headerBytes.Length);
            if (body != null)
            {
                stream.Write(body, 0, body.Length);
            }
        }
    }
}
