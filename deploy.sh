cd crates/server
cargo sqlx prepare
cd ../../
password=$(aws ecr get-login-password --region us-east-1)
echo $password | docker login --username AWS --password-stdin 414942293597.dkr.ecr.us-east-1.amazonaws.com
docker buildx build --platform linux/amd64 -t walnut-server-pipeline .
docker tag walnut-server-pipeline:latest 414942293597.dkr.ecr.us-east-1.amazonaws.com/walnut-server-pipeline:latest
docker push 414942293597.dkr.ecr.us-east-1.amazonaws.com/walnut-server-pipeline:latest
aws ecs update-service --force-new-deployment --service WalnutServer-east-1 --cluster WalnutServer-east-1