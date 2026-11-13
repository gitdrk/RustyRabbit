#!/bin/bash
# Tests karotzd's D-Bus client against a real dbus-daemon in WSL, with karotzd
# running under qemu-arm -cpu ti925t (ARMv4T). Uses a private bus socket in /tmp,
# so fake RFID signals here can't reach anything real.
set -uo pipefail
command -v dbus-daemon >/dev/null || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq dbus-daemon dbus-bin >/dev/null 2>&1
BIN=/mnt/c/Code/Karotz/sim/out/karotzd
SOCK=/tmp/karotzd-test-bus
CONF=/tmp/karotzd-test-bus.conf
PORT=18080
cat > $CONF <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>system</type>
  <listen>unix:path=$SOCK</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="*"/><allow own="*"/>
    <allow send_destination="*"/><allow receive_sender="*"/>
  </policy>
</busconfig>
EOF
rm -f $SOCK
DPID=$(dbus-daemon --config-file=$CONF --fork --print-pid)
sleep 0.5
echo "dbus-daemon $(dbus-daemon --version | head -1 | awk '{print $NF}') pid $DPID"
cd /tmp
echo "--- method calls"
timeout 30 qemu-arm -cpu ti925t $BIN dbus-test $SOCK --emit

echo "--- events: karotzd serve with the listener on the test bus"
qemu-arm -cpu ti925t $BIN serve $PORT --readonly --datadir /tmp/no-such-dir --upstream 127.0.0.1:1 --bus $SOCK > /tmp/karotzd-serve.log 2>&1 &
SPID=$!
sleep 2
echo "before: $(curl -s "http://127.0.0.1:$PORT/api/events")"
curl -sN --max-time 6 "http://127.0.0.1:$PORT/api/events/stream" > /tmp/karotzd-sse.txt &
CPID=$!
sleep 1
# 1) karotzd's own harmless test signal
timeout 30 qemu-arm -cpu ti925t $BIN dbus-test $SOCK --emit | grep emitted
# 2) a foreign sender: a fake RFID tag_in / tag_out, as the rabbit's rfid-daemon would send
DBUS_SYSTEM_BUS_ADDRESS=unix:path=$SOCK dbus-send --system --type=signal /com/mindscape/karotz/Rfid \
  com.mindscape.karotz.KarotzInterface.tag_in string:d0021a0353 string:1 int32:0 int32:3 int32:0 int32:5
DBUS_SYSTEM_BUS_ADDRESS=unix:path=$SOCK dbus-send --system --type=signal /com/mindscape/karotz/Rfid \
  com.mindscape.karotz.KarotzInterface.tag_out string:d0021a0353
# 3) a fake button click, and a signal that should be ignored
DBUS_SYSTEM_BUS_ADDRESS=unix:path=$SOCK dbus-send --system --type=signal /com/mindscape/karotz/Button \
  com.mindscape.karotz.KarotzInterface.click
DBUS_SYSTEM_BUS_ADDRESS=unix:path=$SOCK dbus-send --system --type=signal /org/other/Thing org.other.Iface.noise
sleep 1
echo "after (poll since=0):"
curl -s "http://127.0.0.1:$PORT/api/events?since=0"
echo "long-poll with nothing new (wait=2):"
T0=$(date +%s.%N); R=$(curl -s "http://127.0.0.1:$PORT/api/events?since=4&wait=2"); T1=$(date +%s.%N)
echo "  $R after $(echo "$T1 - $T0" | bc) s"
wait $CPID
echo "SSE stream received:"
grep -E '^(id|data):' /tmp/karotzd-sse.txt | cut -c1-140
echo "server log:"
cat /tmp/karotzd-serve.log | cut -c1-140
kill $SPID $DPID 2>/dev/null
rm -f $SOCK $CONF /tmp/karotzd-sse.txt /tmp/karotzd-serve.log
