package main

import (
	"context"
	"errors"
	"log"

	"github.com/quic-go/quic-go"
)

// Serve 接受连接并为每条起一个回显循环。
func Serve(ln *quic.Listener) {
	connID := 0
	for {
		conn, err := ln.Accept(context.Background())
		if err != nil {
			log.Printf("accept failed: %v", err)
			return
		}
		connID++
		log.Printf("connection %d opened", connID)
		go echoDatagrams(conn, connID)
	}
}

// echoDatagrams 把收到的每个 datagram 原样发回。
// 不解析、不统计、不重排 —— 服务端保持无状态，所有测量都在客户端完成。
func echoDatagrams(conn quic.Connection, connID int) {
	for {
		b, err := conn.ReceiveDatagram(context.Background())
		if err != nil {
			var appErr *quic.ApplicationError
			if errors.As(err, &appErr) && appErr.ErrorCode == 0 {
				log.Printf("connection %d closed cleanly", connID)
			} else {
				log.Printf("connection %d ended: %v", connID, err)
			}
			return
		}
		if err := conn.SendDatagram(b); err != nil {
			log.Printf("connection %d send failed: %v", connID, err)
			return
		}
	}
}
