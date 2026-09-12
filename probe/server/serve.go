package main

import (
	"context"
	"errors"
	"log"

	"github.com/quic-go/quic-go"
)

// Serve 接受连接并为每条起一个回显循环。
func Serve(ln *quic.Listener) {
	for {
		conn, err := ln.Accept(context.Background())
		if err != nil {
			log.Printf("accept failed: %v", err)
			return
		}
		log.Printf("connection from %s", conn.RemoteAddr())
		go echoDatagrams(conn)
	}
}

// echoDatagrams 把收到的每个 datagram 原样发回。
// 不解析、不统计、不重排 —— 服务端保持无状态，所有测量都在客户端完成。
func echoDatagrams(conn quic.Connection) {
	for {
		b, err := conn.ReceiveDatagram(context.Background())
		if err != nil {
			var appErr *quic.ApplicationError
			if errors.As(err, &appErr) && appErr.ErrorCode == 0 {
				log.Printf("connection from %s closed cleanly", conn.RemoteAddr())
			} else {
				log.Printf("connection from %s ended: %v", conn.RemoteAddr(), err)
			}
			return
		}
		if err := conn.SendDatagram(b); err != nil {
			log.Printf("send to %s failed: %v", conn.RemoteAddr(), err)
			return
		}
	}
}
