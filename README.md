# Rust Server Monitor

A high-performance, full-featured server monitoring system built in Rust. This system collects, stores, and visualizes system metrics from multiple servers with real-time dashboard, historical analysis, and responsive design.

## Features

- **High Performance**: Built with Rust for maximum speed and reliability
- **Real-time Monitoring**: Live dashboard showing all server metrics
- **Historical Data**: Charts and analytics for 1 hour, 24 hours, and 7 days
- **Multi-server Support**: Monitor multiple servers simultaneously
- **Modern UI**: Bootstrap 5 responsive design with Chart.js visualizations
- **Async Architecture**: Non-blocking concurrent metric collection
- **Docker Support**: Ready-to-use containerized deployment
- **Database Options**: SQLite (default) or PostgreSQL support

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Web Frontend  │    │   Actix-web     │    │   SQLite/PG     │
│  (Dashboard)    │◄──►│   API Server    │◄──►│   Database      │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │
                              ▼
                       ┌──────────────────┐
                       │  Async Scheduler │
                       │  (Tokio Tasks)   │
                       └──────────────────┘
                              │
                              ▼
                       ┌──────────────────┐
                       │  Metrics Service│
                       │  (Netdata API)  │
                       └──────────────────┘
                              │
                              ▼
                       ┌──────────────────┐
                       │  Target Servers │
                       │  (Netdata:19999)│
                       └──────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.75+ (for local development)
- Docker & Docker Compose (for containerized deployment)
- Target servers must have [Netdata](https://github.com/netdata/netdata) installed and running on port 19999

### Using Docker (Recommended)

1. **Clone and Build**
   ```bash
   git clone <repository-url>
   cd rust-monitor-server
   docker-compose up -d
   ```

2. **Access Dashboard**
   Open http://localhost:5400 in your browser

3. **Add Servers**
   Click "Add Server" and provide:
   - Server name
   - IP address (must have Netdata accessible on port 19999)

### Local Development

1. **Install Dependencies**
   ```bash
   # Install Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   
   # Install SQLite development libraries (Ubuntu/Debian)
   sudo apt-get install libsqlite3-dev pkg-config libssl-dev
   ```

2. **Setup Environment**
   ```bash
   cp .env.example .env
   # Edit .env as needed
   ```

3. **Run Application**
   ```bash
   cargo run
   ```

4. **Access Dashboard**
   Open http://localhost:5400

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `sqlite:./monitor.db` | Database connection string |
| `SERVER_PORT` | `5400` | Web server port |
| `COLLECTION_INTERVAL` | `30` | Metrics collection interval (seconds) |
| `RUST_LOG` | `info` | Log level |

### Database Options

#### SQLite (Default)
```env
DATABASE_URL=sqlite:./monitor.db
```

#### PostgreSQL
```env
DATABASE_URL=postgresql://username:password@localhost/monitor_db
```

To use PostgreSQL with Docker Compose:
```bash
docker-compose --profile postgres up -d
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/` | Main dashboard |
| GET | `/servers` | List all servers |
| POST | `/servers` | Add new server |
| DELETE | `/servers/{id}` | Remove server |
| GET | `/history/{server_id}?period=1h|24h|7d` | Historical metrics |
| GET | `/api/average/{server_id}?period=1h|24h|7d` | Average metrics |

### Add Server Example
```bash
curl -X POST http://localhost:5400/servers \
  -H "Content-Type: application/json" \
  -d '{"name": "Web Server", "ip_address": "192.168.1.100"}'
```

## Target Server Setup

### Install Netdata

On Ubuntu/Debian:
```bash
bash <(curl -Ss https://my-netdata.io/kickstart.sh)
```

On CentOS/RHEL:
```bash
bash <(curl -Ss https://my-netdata.io/kickstart.sh) --stable-channel
```

### Verify Netdata
```bash
curl http://localhost:19999/api/v1/info
```

### Firewall Configuration
Ensure port 19999 is accessible from the monitor server:
```bash
sudo ufw allow from <monitor-ip> to any port 19999
```

## Metrics Collected

- **CPU Usage**: Percentage of CPU utilization
- **Memory Usage**: Used memory percentage and total GB
- **Disk Usage**: Disk utilization percentage
- **Load Average**: System load average
- **Logged Users**: Number of active user sessions
- **Network I/O**: Network in/out throughput (KB/s)
- **Uptime**: System uptime in seconds

## Dashboard Features

### Real-time Metrics
- Live progress bars for CPU, memory, and disk usage
- Color-coded status indicators
- Automatic refresh every 30 seconds

### Historical Charts
- Interactive Chart.js visualizations
- Multiple time periods (1h, 24h, 7d)
- Separate charts for each metric type

### Server Management
- Add/remove servers through web interface
- Server status indicators (online/offline)
- Last seen timestamps

## Performance & Scalability

### Concurrency
- Async Tokio runtime for non-blocking operations
- Concurrent metric collection from multiple servers
- Connection pooling for database operations

### Resource Usage
- Minimal memory footprint
- Efficient CPU utilization
- Configurable collection intervals

### Scaling
- Horizontal scaling with PostgreSQL
- Load balancer compatible
- Container-ready deployment

## Troubleshooting

### Common Issues

1. **Server Shows Offline**
   - Verify Netdata is running: `systemctl status netdata`
   - Check port accessibility: `telnet <server-ip> 19999`
   - Review application logs

2. **Database Connection Errors**
   - Verify DATABASE_URL in .env
   - Check database file permissions
   - Ensure SQLite development libraries are installed

3. **Template Errors**
   - Verify templates directory exists
   - Check template syntax
   - Review Tera compilation logs

### Logs

View application logs:
```bash
# Docker
docker-compose logs -f monitor-server

# Local development
RUST_LOG=debug cargo run
```

### Health Checks

The application includes health checks:
```bash
curl http://localhost:5400/servers
```

## Development

### Project Structure
```
src/
├── main.rs              # Application entry point
├── models/              # Database models
│   └── mod.rs
├── routes/              # Web routes and handlers
│   └── mod.rs
├── services/            # Business logic
│   └── mod.rs           # Metrics collection service
└── scheduler/           # Background tasks
    └── mod.rs           # Async scheduler

templates/               # HTML templates
├── base.html           # Base template
└── dashboard.html      # Main dashboard

migrations/             # Database migrations
└── 001_create_tables.sql
```

### Adding New Metrics

1. Update `models/mod.rs` with new fields
2. Modify `services/mod.rs` extraction methods
3. Update database schema in migrations
4. Enhance dashboard template

### Testing

Run tests:
```bash
cargo test
```

## Production Deployment

### Docker Production

1. **Build Image**
   ```bash
   docker build -t rust-monitor .
   ```

2. **Run with Data Persistence**
   ```bash
   docker run -d \
     --name monitor \
     -p 5400:5400 \
     -v ./data:/app/data \
     -e DATABASE_URL=sqlite:/app/data/monitor.db \
     rust-monitor
   ```

### Security Considerations

- Use reverse proxy (nginx/caddy) for SSL termination
- Implement authentication for production
- Restrict database file permissions
- Use firewall rules to limit access
- Regular security updates

## Contributing

1. Fork the repository
2. Create feature branch
3. Make changes with tests
4. Submit pull request

## License

This project is licensed under the MIT License.

## Support

For issues and questions:
- Create GitHub issue
- Check troubleshooting section
- Review application logs

---

**Built with ❤️ using Rust, Actix-web, and Netdata**
