module.exports = {
  apps: [{
    name: 'rust-monitor',
    script: './target/release/rust-monitor-server',
    instances: 2, // Cluster mode for better performance
    exec_mode: 'cluster',
    
    // Environment variables
    env: {
      NODE_ENV: 'development',
      DATABASE_URL: 'sqlite:./monitor.db',
      SERVER_PORT: 5400,
      RUST_LOG: 'debug'
    },
    
    env_production: {
      NODE_ENV: 'production',
      DATABASE_URL: 'sqlite:/var/www/monitor/data/monitor.db',
      SERVER_PORT: 5401,
      RUST_LOG: 'info',
      COLLECTION_INTERVAL: 60,
      AUTH_USERNAME: 'servers',
      AUTH_PASSWORD: 'Soft@26*'
    },
    
    // Log files
    error_file: '/var/log/monitor/error.log',
    out_file: '/var/log/monitor/out.log',
    log_file: '/var/log/monitor/combined.log',
    time: true,
    
    // Process management
    max_memory_restart: '1G',
    min_uptime: '10s',
    max_restarts: 10,
    
    // Monitoring
    watch: false, // Don't watch files in production
    ignore_watch: ['node_modules', 'logs'],
    
    // Graceful shutdown
    kill_timeout: 5000,
    
    // Health check
    health_check_grace_period: 3000,
    
    // Auto restart on file changes (only in development)
    watch_options: {
      followSymlinks: false
    }
  }]
};
