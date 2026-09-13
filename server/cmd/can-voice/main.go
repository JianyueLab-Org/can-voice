// can-voice 是 Cerulean Aviation Network 的语音服务端。
//
// 无状态：没有数据库、没有持久化、没有 ACL。进程重启等于所有人重连并重发 SUB。
// 这也意味着没有"丢了某个卷就全网瘫痪且无法远程修复"的风险（spec 8）。
package main

import (
	"context"
	"crypto/tls"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/JianyueLab-Org/can-voice/server/internal/fsdfeed"
	"github.com/JianyueLab-Org/can-voice/server/internal/router"
	"github.com/JianyueLab-Org/can-voice/server/internal/transport"
)

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{
		Level: levelFromEnv(),
	})))

	cfg, err := LoadConfig(os.Getenv)
	if err != nil {
		slog.Error("configuration is incomplete", "error", err)
		os.Exit(1)
	}
	cert, err := tls.LoadX509KeyPair(cfg.Cert, cfg.Key)
	if err != nil {
		slog.Error("cannot load the TLS key pair", "error", err)
		os.Exit(1)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	r := router.New()
	feed := fsdfeed.NewFeed(cfg.FeedURL)
	r.SetLocator(feed)
	go feed.Run(ctx)

	slog.Info("starting", "addr", cfg.Addr, "feed", cfg.FeedURL, "max_rx", cfg.MaxRX)
	if err := transport.Serve(ctx, transport.Config{
		Addr:      cfg.Addr,
		TLS:       &tls.Config{Certificates: []tls.Certificate{cert}},
		PublicKey: cfg.PubKey,
		MaxRX:     cfg.MaxRX,
	}, r); err != nil && ctx.Err() == nil {
		slog.Error("server stopped", "error", err)
		os.Exit(1)
	}
	slog.Info("shut down cleanly")
}

func levelFromEnv() slog.Level {
	if os.Getenv("CAN_VOICE_DEBUG") != "" {
		return slog.LevelDebug
	}
	return slog.LevelInfo
}
