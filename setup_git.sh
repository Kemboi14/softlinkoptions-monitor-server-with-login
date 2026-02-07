#!/bin/bash

echo "Setting up git repository..."
cd "/home/nick/rust monitor server"

# Initialize git if not already done
if [ ! -d .git ]; then
    git init
    echo "Git repository initialized"
fi

# Add all files
git add .
echo "Files added"

# Commit changes
git commit -m "Initial commit of rust server monitoring system"
echo "Changes committed"

# Add remote origin
git remote add origin https://github.com/Kemboi14/rust-server-monitoring-system.git 2>/dev/null || echo "Remote already exists"

# Create and checkout new branch
git checkout -b rust-server-monitor-second 2>/dev/null || git checkout rust-server-monitor-second
echo "Branch rust-server-monitor-second created/checked out"

# Try to push the branch
echo "Attempting to push branch..."
git push -u origin rust-server-monitor-second 2>&1

echo "Git setup complete!"
echo "Current branch: $(git branch --show-current)"
echo "Remote status:"
git remote -v 2>/dev/null || echo "No remotes configured"
