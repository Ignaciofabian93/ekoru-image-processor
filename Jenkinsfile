// Writes this service's env file for one environment from Infisical (INF-14). The CLI runs
// as a one-off container on the infisical-clients network, so nothing is installed in
// Jenkins. The folder in Infisical is named after SERVICE_NAME. If the file lacks the key
// the service always has, the deploy stops before anything is built.
def exportEnvFromInfisical(String environment, String envFile, String requiredKey) {
  withCredentials([
    string(credentialsId: 'infisical-client-id', variable: 'INFISICAL_UNIVERSAL_AUTH_CLIENT_ID'),
    string(credentialsId: 'infisical-client-secret', variable: 'INFISICAL_UNIVERSAL_AUTH_CLIENT_SECRET'),
    string(credentialsId: 'infisical-project-id', variable: 'INFISICAL_PROJECT_ID')
  ]) {
    withEnv(["ENV_SLUG=${environment}", "ENV_FILE=${envFile}", "REQUIRED_KEY=${requiredKey}"]) {
      sh '''
        set +x  # keeps the access token out of the build log
        RUN="docker run --rm --network infisical-clients -e INFISICAL_DOMAIN=http://infisical:8080"
        IMAGE=infisical/cli:0.44.1
        INFISICAL_TOKEN=$($RUN -e INFISICAL_UNIVERSAL_AUTH_CLIENT_ID -e INFISICAL_UNIVERSAL_AUTH_CLIENT_SECRET \
          $IMAGE login --method=universal-auth --silent --plain)
        export INFISICAL_TOKEN
        # --expand=false: values are written exactly as stored, so a "$" in a password stays.
        $RUN -e INFISICAL_TOKEN -e INFISICAL_PROJECT_ID $IMAGE \
          sm export --env="$ENV_SLUG" --path="/$SERVICE_NAME" --format=dotenv --expand=false --silent \
          > "$ENV_FILE"
        grep -q "^$REQUIRED_KEY=" "$ENV_FILE" \
          || { echo "The Infisical export for $SERVICE_NAME ($ENV_SLUG) has no $REQUIRED_KEY. Not deploying."; exit 1; }
      '''
    }
  }
}

// Waits for the container's healthcheck (compose.*.yml) to pass. With a missing or bad env
// value the service exits at startup and `restart: always` brings it back forever, while
// `up -d` still reports success. Startup errors name the variable, never its value, so the
// log tail is safe to print.
def waitHealthy(String container) {
  withEnv(["CONTAINER=${container}"]) {
    sh '''
      for i in $(seq 1 24); do
        STATUS=$(docker inspect -f '{{.State.Health.Status}}' "$CONTAINER" 2>/dev/null || echo missing)
        [ "$STATUS" = healthy ] && { echo "$CONTAINER is healthy."; exit 0; }
        sleep 5
      done
      echo "$CONTAINER is not healthy after 2 minutes (last status: $STATUS). Last log lines:"
      docker logs --tail 40 "$CONTAINER" 2>&1
      exit 1
    '''
  }
}

pipeline {
  agent none

  environment {
    SERVICE_NAME = 'ekoru-image-processor'
  }

  // Build records and logs are kept for the last 20 runs only (INF-15).
  options {
    buildDiscarder(logRotator(numToKeepStr: '20', artifactNumToKeepStr: '5'))
  }

  stages {

    // Prevents the version-bump commit from re-triggering the full pipeline
    stage('Skip CI check') {
      agent any
      steps {
        script {
          def msg = sh(script: 'git log -1 --pretty=%B', returnStdout: true).trim()
          if (msg.contains('[skip ci]')) {
            currentBuild.result = 'NOT_BUILT'
            error('Version bump commit — skipping pipeline.')
          }
        }
      }
    }

    // Formatting and clippy with warnings as errors, in the Rust image the Dockerfile builds
    // with. The named volume keeps downloaded crates between runs, and target/ stays in the
    // workspace, so later runs only recompile what changed. --locked stops the run if
    // Cargo.lock is out of date instead of resolving new versions here.
    stage('Checks') {
      agent {
        docker {
          image 'rust:1.94-slim-bookworm'
          args '-u root -v ekoru-cargo-registry:/usr/local/cargo/registry'
        }
      }
      steps {
        sh 'rustup component add rustfmt clippy'
        sh 'cargo fmt --check'
        sh 'cargo clippy --all-targets --locked -- -D warnings'
      }
    }

    // The unit and handler tests (IMG-2) run against an in-memory store, with no R2.
    stage('Test') {
      agent {
        docker {
          image 'rust:1.94-slim-bookworm'
          args '-u root -v ekoru-cargo-registry:/usr/local/cargo/registry'
        }
      }
      steps {
        sh 'cargo test --locked'
      }
    }

    // ── Staging flow ──────────────────────────────────────────────────────────

    stage('Deploy Staging') {
      agent any
      when { branch 'main' }
      steps {
        script { exportEnvFromInfisical('staging', '.env.staging', 'INTERNAL_TOKEN') }
        // Unlike the Node services, this build keeps the layer cache: the Dockerfile compiles
        // the dependencies in their own layer, which saves minutes whenever Cargo.toml and
        // Cargo.lock are unchanged. --pull still fetches patched base images.
        sh '''
          docker compose -f compose.staging.yml build --pull
          docker compose -f compose.staging.yml up -d --force-recreate
        '''
        script { waitHealthy('ekoru-image-processor-staging') }
        sh '''
          docker image prune -f
          # The build cache grows on every deploy until the disk fills (INF-15). Keep the
          # last day only; -a, because without it prune only removes dangling records.
          docker builder prune -af --filter until=24h
        '''
        sshagent(['github-deploy-key-image-processor']) {
          sh '''
            git remote set-url origin "$(git remote get-url origin | sed 's|https://github.com/|git@github.com:|')"
            VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
            git tag -f "staging/v${VERSION}"
            git push -f origin "staging/v${VERSION}"
          '''
        }
      }
    }

    stage('Confirm E2E OK') {
      agent none
      when { branch 'main' }
      steps {
        timeout(time: 24, unit: 'HOURS') {
          input message: "Staging deployed for ${SERVICE_NAME}. E2E tests passed?",
                ok: 'Yes, deploy to production'
        }
      }
    }

    // ── Production deploy ─────────────────────────────────────────────────────

    stage('Deploy Production') {
      agent any
      when { branch 'main' }
      steps {
        script { exportEnvFromInfisical('prod', '.env.prod', 'INTERNAL_TOKEN') }
        // Same source as staging, so the cached layers make this build quick.
        sh '''
          docker compose -f compose.prod.yml build --pull
          docker compose -f compose.prod.yml up -d --force-recreate
        '''
        script { waitHealthy('ekoru-image-processor') }
        sh '''
          docker image prune -f
          docker builder prune -af --filter until=24h
        '''
        sshagent(['github-deploy-key-image-processor']) {
          sh '''
            git remote set-url origin "$(git remote get-url origin | sed 's|https://github.com/|git@github.com:|')"
            VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
            git tag -f "prod/v${VERSION}"
            git push -f origin "prod/v${VERSION}"
          '''
        }
      }
    }

  }

  post {
      failure {
          echo "Pipeline failed for ${SERVICE_NAME} on branch ${env.BRANCH_NAME}"
      }
      success {
          echo "Pipeline completed for ${SERVICE_NAME} on ${env.BRANCH_NAME}"
      }
  }
}
